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

fn select_rating<'a, T, FCountry, FRating>(
    ratings: &'a [T],
    metadata_country: &str,
    country: FCountry,
    rating: FRating,
) -> Option<(String, String)>
where
    FCountry: Fn(&'a T) -> &'a str,
    FRating: Fn(&'a T) -> Option<&'a str>,
{
    let valid = |item: &'a T| {
        rating(item)
            .map(str::trim)
            .filter(|rating| !rating.is_empty())
            .map(|rating| (country(item).to_string(), rating.to_string()))
    };

    ratings
        .iter()
        .find(|item| {
            country(item).eq_ignore_ascii_case(metadata_country)
                && valid(item).is_some()
        })
        .and_then(valid)
        .or_else(|| {
            ratings
                .iter()
                .find(|item| {
                    country(item).eq_ignore_ascii_case("US") && valid(item).is_some()
                })
                .and_then(valid)
        })
        .or_else(|| ratings.iter().find_map(valid))
}

fn tmdb_rating_label(country: &str, rating: &str) -> String {
    if country.eq_ignore_ascii_case("US") {
        rating.to_string()
    } else if country.eq_ignore_ascii_case("DE")
        && !rating.to_uppercase().starts_with("FSK")
    {
        format!("FSK-{rating}")
    } else {
        format!("{}-{rating}", country.to_uppercase())
    }
}

fn rating_age(label: &str, country: &str) -> Option<i32> {
    crate::localization::ratings::resolve_rating_age(Some(label), Some(country))
        .or_else(|| crate::localization::ratings::resolve_rating_age(Some(label), None))
}

fn build_person_relations_tmdb(
    left_media_id: uuid::Uuid,
    credits: &sdks::tmdb::Credits,
) -> Vec<super::MetaRelation> {
    let mut relations = Vec::new();

    // Cast (actors)
    for (i, member) in credits.cast.iter().enumerate() {
        let name = &member.name;
        let person_id =
            utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
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
            "Producer" | "Executive Producer" | "Co-Producer" => {
                Some(db::RelationRole::Producer)
            }
            _ => None,
        };

        if let Some(role) = role {
            let name = &member.name;
            let person_id =
                utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
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
        let metadata_country = config
            .metadata_country_code
            .as_deref()
            .map(db::normalize_country_alpha2)
            .unwrap_or_else(|| "US".to_string());

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
                        .execute(
                            sdks::tmdb::MovieEndpoint::new(m.id)
                                .with_cache(Duration::from_secs(3600)),
                        )
                        .await?;

                    let external_ids = db::ExternalIds {
                        tmdb: Some(movie_details.id),
                        imdb: movie_details.imdb_id.clone().or(ids.imdb.clone()),
                        tvdb: ids.tvdb,
                    };
                    let logo = movie_details
                        .images
                        .as_ref()
                        .and_then(|i| i.best_logo())
                        .and_then(|p| tmdb_image(Some(p)));
                    let rating = movie_details.release_dates.as_ref().and_then(
                        |release_dates| {
                            let releases = release_dates
                                .results
                                .iter()
                                .flat_map(|country| {
                                    country.release_dates.iter().map(|release| {
                                        (
                                            country.iso_3166_1.as_str(),
                                            release.certification.as_deref(),
                                        )
                                    })
                                })
                                .collect::<Vec<_>>();
                            select_rating(
                                &releases,
                                &metadata_country,
                                |(country, _)| country,
                                |(_, certification)| *certification,
                            )
                        },
                    );
                    let (certification, certification_age) = rating
                        .map(|(country, rating)| {
                            let label = tmdb_rating_label(&country, &rating);
                            let age = rating_age(&label, &country);
                            (Some(label), age)
                        })
                        .unwrap_or((None, None));
                    let mut result_media = db::Media {
                        title: movie_details.title,
                        description: movie_details.overview,
                        released_at: movie_details
                            .release_date
                            .and_then(|d| d.and_hms_opt(0, 0, 0)),
                        runtime: movie_details.runtime.map(|r| r * 60),
                        rating_audience: movie_details.vote_average,
                        poster: tmdb_image(movie_details.poster_path.as_deref()),
                        backdrop: tmdb_image(movie_details.backdrop_path.as_deref()),
                        logo,
                        certification,
                        certification_age,
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
                        season_posters: std::collections::HashMap::new(),
                    }));
                }
            }
            db::MediaKind::Series => {
                let s = find_resp.tv_results.into_iter().next();
                if let Some(s) = s {
                    let tv_details = client
                        .execute(
                            sdks::tmdb::SeriesEndpoint::new(s.id)
                                .with_cache(Duration::from_secs(3600)),
                        )
                        .await?;

                    let external_ids = db::ExternalIds {
                        tmdb: Some(tv_details.id),
                        imdb: ids.imdb.clone(),
                        tvdb: ids.tvdb,
                    };
                    let country = tv_details.origin_country.into_iter().next();
                    let logo = tv_details
                        .images
                        .as_ref()
                        .and_then(|i| i.best_logo())
                        .and_then(|p| tmdb_image(Some(p)));
                    let rating = tv_details.content_ratings.as_ref().and_then(
                        |content_ratings| {
                            select_rating(
                                &content_ratings.results,
                                &metadata_country,
                                |rating| rating.iso_3166_1.as_str(),
                                |rating| rating.rating.as_deref(),
                            )
                        },
                    );
                    let (certification, certification_age) = rating
                        .map(|(country, rating)| {
                            let label = tmdb_rating_label(&country, &rating);
                            let age = rating_age(&label, &country);
                            (Some(label), age)
                        })
                        .unwrap_or((None, None));
                    let mut result_media = db::Media {
                        title: tv_details.name,
                        description: tv_details.overview,
                        released_at: tv_details
                            .first_air_date
                            .and_then(|d| d.and_hms_opt(0, 0, 0)),
                        rating_audience: tv_details.vote_average,
                        poster: tmdb_image(tv_details.poster_path.as_deref()),
                        backdrop: tmdb_image(tv_details.backdrop_path.as_deref()),
                        logo,
                        certification,
                        certification_age,
                        country,
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
                            let person_id = crate::utils::get_stable_uuid(format!(
                                "person:{}",
                                name.to_lowercase()
                            ));
                            relations.push(super::MetaRelation {
                                media: db::Media {
                                    id: person_id,
                                    title: name.clone(),
                                    kind: db::MediaKind::Person,
                                    poster: tmdb_image(creator.profile_path.as_deref()),
                                    media_id: Some(format!(
                                        "person:{}",
                                        name.to_lowercase()
                                    )),
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

                    let season_posters: std::collections::HashMap<i64, String> =
                        tv_details
                            .seasons
                            .iter()
                            .filter_map(|s| {
                                let poster = tmdb_image(s.poster_path.as_deref())?;
                                Some((s.season_number, poster))
                            })
                            .collect();

                    return Ok(Some(MetaResult {
                        media: result_media,
                        relations,
                        season_posters,
                    }));
                }
            }
            db::MediaKind::Episode => {
                // For episodes, we need series_tmdb_id, season_number, and episode_number
                let series_tmdb_id = if let Some(sid) = media.series_id {
                    let s = db::Media::get_by_id(&ctx.db, &sid)
                        .await?
                        .filter(|m| m.kind == db::MediaKind::Series);
                    s.and_then(|m| m.external_ids.tmdb)
                } else {
                    None
                };

                let season_number = media.parent_idx;
                let episode_number = media.idx;

                if let (Some(tmdb_id), Some(s_n), Some(e_n)) =
                    (series_tmdb_id, season_number, episode_number)
                {
                    let ep_details = client
                        .execute(
                            sdks::tmdb::EpisodeEndpoint::new(tmdb_id, s_n, e_n)
                                .with_cache(Duration::from_secs(3600)),
                        )
                        .await?;

                    let external_ids = db::ExternalIds {
                        tmdb: Some(ep_details.id),
                        imdb: ep_details
                            .external_ids
                            .and_then(|e| e.imdb_id)
                            .or(ids.imdb.clone()),
                        tvdb: ids.tvdb,
                    };
                    let mut result_media = db::Media {
                        title: ep_details.name,
                        description: ep_details.overview,
                        released_at: ep_details
                            .air_date
                            .and_then(|d| d.and_hms_opt(0, 0, 0)),
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
                        season_posters: std::collections::HashMap::new(),
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
