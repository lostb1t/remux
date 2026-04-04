use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use tracing::warn;

use crate::sdks::CachedEndpoint;
use crate::{AppContext, db, sdks, utils};

use super::{MetaProvider, MetaResult};

pub struct TmdbMetaProvider;

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

fn tmdb_image(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", TMDB_IMAGE_BASE, p))
}

fn build_person_relations_tmdb(
    left_media_id: uuid::Uuid,
    credits: &sdks::tmdb::Credits,
) -> Vec<super::MetaRelation> {
    let mut relations = Vec::new();

    // Cast (actors)
    for (i, member) in credits.cast.iter().enumerate() {
        let name = &member.name;
        let person_id = utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push(super::MetaRelation {
            media: db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                poster: tmdb_image(member.profile_path.as_deref()),
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
            relation: db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(i as i64),
                role: Some(db::RelationRole::Actor),
                character: member.character.clone(),
                ..Default::default()
            },
        });
    }

    // Crew (directors and writers)
    for (i, member) in credits.crew.iter().enumerate() {
        let role = match member.job.as_str() {
            "Director" => Some(db::RelationRole::Director),
            "Writer" | "Screenplay" | "Author" => Some(db::RelationRole::Writer),
            "Producer" | "Executive Producer" | "Co-Producer" => Some(db::RelationRole::Producer),
            _ => None,
        };

        if let Some(role) = role {
            let name = &member.name;
            let person_id = utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
            relations.push(super::MetaRelation {
                media: db::Media {
                    id: person_id,
                    title: name.clone(),
                    kind: db::MediaKind::Person,
                    poster: tmdb_image(member.profile_path.as_deref()),
                    media_id: Some(format!("person:{}", name.to_lowercase())),
                    ..Default::default()
                },
                relation: db::MediaRelation {
                    left_media_id,
                    right_media_id: person_id,
                    weight: Some(i as i64),
                    role: Some(role),
                    ..Default::default()
                },
            });
        }
    }

    relations
}

#[async_trait]
impl MetaProvider for TmdbMetaProvider {
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let api_key = config.get_tmdb_key().to_string();

        let ids = &media.external_ids;
        let lookup = if let Some(tmdb_id) = ids.tmdb {
            Some((tmdb_id.to_string(), "tmdb_id"))
        } else if let Some(ref imdb) = ids.imdb {
            Some((imdb.clone(), "imdb_id"))
        } else if let Some(ref series_id) = media.series_media_id {
            // series_media_id is the parent series' aio_id; for series that is the imdb id
            Some((series_id.clone(), "imdb_id"))
        } else if let Some(tvdb_id) = ids.tvdb {
            Some((tvdb_id.to_string(), "tvdb_id"))
        } else {
            None
        };

        let (external_id, external_source) = match lookup {
            Some(pair) => pair,
            None => return Ok(None),
        };

        let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
            .with_auth(sdks::BearerAuth { token: api_key });

        let find_resp = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id,
                    external_source: external_source.to_string(),
                }
                .with_cache(Duration::from_secs(3600)),
            )
            .await;

        let find_resp = match find_resp {
            Ok(r) => r,
            Err(e) => {
                warn!("tmdb find error: {e}");
                return Ok(None);
            }
        };

        match media.kind {
            db::MediaKind::Movie => {
                let m = find_resp.movie_results.into_iter().next();
                if let Some(m) = m {
                    let movie_details = client
                        .execute(sdks::tmdb::MovieEndpoint::new(m.id).with_cache(Duration::from_secs(3600)))
                        .await?;
                    
                    let external_ids = db::ExternalIds {
                        tmdb: Some(movie_details.id),
                        imdb: movie_details.imdb_id.clone().or(ids.imdb.clone()),
                        tvdb: ids.tvdb,
                    };
                    let mut result_media = db::Media {
                        title: movie_details.title,
                        description: movie_details.overview,
                        released_at: movie_details.release_date.map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                        runtime: movie_details.runtime.map(|r| r * 60),
                        rating_audience: movie_details.vote_average,
                        poster: tmdb_image(movie_details.poster_path.as_deref()),
                        backdrop: tmdb_image(movie_details.backdrop_path.as_deref()),
                        external_ids: sqlx::types::Json(external_ids),
                        ..Default::default()
                    };
                    
                    let mut relations = vec![];
                    if let Some(credits) = &movie_details.credits {
                        relations = build_person_relations_tmdb(media.id, credits);
                    }
                    
                    return Ok(Some(MetaResult {
                        media: result_media,
                        relations,
                    }));
                }
            }
            db::MediaKind::Series => {
                let s = find_resp.tv_results.into_iter().next();
                if let Some(s) = s {
                    let tv_details = client
                        .execute(sdks::tmdb::SeriesEndpoint::new(s.id).with_cache(Duration::from_secs(3600)))
                        .await?;
                    
                    let external_ids = db::ExternalIds {
                        tmdb: Some(tv_details.id),
                        imdb: ids.imdb.clone(),
                        tvdb: ids.tvdb,
                    };
                    let mut result_media = db::Media {
                        title: tv_details.name,
                        description: tv_details.overview,
                        released_at: tv_details.first_air_date.map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                        rating_audience: tv_details.vote_average,
                        poster: tmdb_image(tv_details.poster_path.as_deref()),
                        backdrop: tmdb_image(tv_details.backdrop_path.as_deref()),
                        external_ids: sqlx::types::Json(external_ids),
                        ..Default::default()
                    };
                    
                    let mut relations = vec![];
                    if let Some(credits) = &tv_details.credits {
                        relations = build_person_relations_tmdb(media.id, credits);
                    }

                    if let Some(creators) = &tv_details.created_by {
                        for (i, creator) in creators.iter().enumerate() {
                            let name = &creator.name;
                            let person_id = crate::utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
                            relations.push(super::MetaRelation {
                                media: db::Media {
                                    id: person_id,
                                    title: name.clone(),
                                    kind: db::MediaKind::Person,
                                    poster: tmdb_image(creator.profile_path.as_deref()),
                                    media_id: Some(format!("person:{}", name.to_lowercase())),
                                    ..Default::default()
                                },
                                relation: db::MediaRelation {
                                    left_media_id: media.id,
                                    right_media_id: person_id,
                                    weight: Some(i as i64),
                                    role: Some(db::RelationRole::Creator),
                                    ..Default::default()
                                },
                            });
                        }
                    }
                    
                    return Ok(Some(MetaResult {
                        media: result_media,
                        relations,
                    }));
                }
            }
            db::MediaKind::Episode => {
                // For episodes, we need series_tmdb_id, season_number, and episode_number
                let series_tmdb_id = if let Some(sid) = media.series_id {
                    let s = db::Media::get_by_id(&ctx.db, &sid).await?.filter(|m| m.kind == db::MediaKind::Series);
                    s.and_then(|m| m.external_ids.tmdb)
                } else {
                    None
                };
                
                let season_number = media.parent_idx;
                let episode_number = media.idx;
                
                if let (Some(tmdb_id), Some(s_n), Some(e_n)) = (series_tmdb_id, season_number, episode_number) {
                    let ep_details = client
                        .execute(sdks::tmdb::EpisodeEndpoint::new(tmdb_id, s_n, e_n).with_cache(Duration::from_secs(3600)))
                        .await?;
                        
                    let external_ids = db::ExternalIds {
                        tmdb: Some(ep_details.id),
                        imdb: ep_details.external_ids.and_then(|e| e.imdb_id).or(ids.imdb.clone()),
                        tvdb: ids.tvdb,
                    };
                    let mut result_media = db::Media {
                        title: ep_details.name,
                        description: ep_details.overview,
                        released_at: ep_details.air_date.map(|d| d.and_hms_opt(0, 0, 0).unwrap()),
                        runtime: ep_details.runtime.map(|r| r * 60),
                        rating_audience: ep_details.vote_average,
                        poster: tmdb_image(ep_details.still_path.as_deref()),
                        external_ids: sqlx::types::Json(external_ids),
                        ..Default::default()
                    };
                    
                    let mut relations = vec![];
                    if let Some(credits) = &ep_details.credits {
                        relations = build_person_relations_tmdb(media.id, credits);
                    }
                    
                    return Ok(Some(MetaResult {
                        media: result_media,
                        relations,
                    }));
                }
            }
            _ => {
                // Fallback for other kinds or if find_resp didn't have matches
            }
        }

        Ok(None)
    }
}
