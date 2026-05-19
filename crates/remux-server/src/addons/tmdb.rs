use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration, MediaKind,
    ResourceType,
};
use crate::sdks::{CachedEndpoint, ClientError};
use crate::{AppContext, api, common, db, sdks};

pub struct TmdbPreset;

impl AddonPreset for TmdbPreset {
    fn id(&self) -> &'static str {
        "tmdb"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "tmdb".to_string(),
            display_name: "TMDB".to_string(),
            description:
                "The Movie Database — high-resolution images, fallback metadata, \
                 and people search."
                    .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Meta, ResourceType::Search],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Series,
                MediaKind::Episode,
                MediaKind::Person,
            ],
            options: vec![],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        _cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
        Ok(Arc::new(TmdbAddon {}))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TmdbPreset))
}

pub struct TmdbAddon {}

const TMDB_IMAGE_BASE: &str = "https://image.tmdb.org/t/p/original";

fn tmdb_image(path: Option<&str>) -> Option<String> {
    path.filter(|p| !p.is_empty())
        .map(|p| format!("{}{}", TMDB_IMAGE_BASE, p))
}

#[async_trait]
impl AddonKind for TmdbAddon {
    fn id(&self) -> &'static str {
        "tmdb"
    }

    async fn meta_supports(&self, media: &db::Media) -> bool {
        matches!(
            media.kind,
            db::MediaKind::Movie
                | db::MediaKind::Series
                | db::MediaKind::Season
                | db::MediaKind::Episode
                | db::MediaKind::Person
        )
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        match fetch_tmdb_meta(media, ctx).await {
            Err(e) if is_404(&e) => Ok(None),
            other => other,
        }
    }

    async fn remote_images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        tmdb_remote_images(ctx, media).await
    }

    async fn search_supports(&self, kind: &db::MediaKind) -> bool {
        matches!(kind, db::MediaKind::Person)
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if !matches!(kind, db::MediaKind::Person) {
            return Ok(None);
        }
        Ok(Some(search_tmdb_person(query, limit, ctx).await?))
    }

    async fn search_persist(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let mut media = match ctx.store.get::<db::Media>(id.to_string()) {
            Some(m) => m,
            None => return Ok(None),
        };
        media.save(&ctx.db).await.ok();
        ctx.store.delete(id.to_string());
        Ok(Some(media))
    }
}

// ---------------------------------------------------------------------------
// TMDB meta fetch
// ---------------------------------------------------------------------------

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

fn build_person_relations(
    left_media_id: uuid::Uuid,
    credits: &sdks::tmdb::Credits,
) -> Vec<(db::MediaRelation, db::Media)> {
    let mut relations = Vec::new();
    for (i, member) in credits.cast.iter().enumerate() {
        let name = &member.name;
        let person_id =
            common::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        let mut person = db::Media {
            id: person_id,
            title: name.clone(),
            kind: db::MediaKind::Person,
            media_id: Some(format!("person:{}", name.to_lowercase())),
            external_ids: db::ExternalIds {
                tmdb: Some(member.id),
                ..Default::default()
            },
            ..Default::default()
        };
        if let Some(url) = tmdb_image(member.profile_path.as_deref()) {
            person.set_image(db::ImageKind::Primary, url);
        }
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(i as i64),
                role: Some(db::RelationRole::Actor),
                character: member.character.clone(),
                ..Default::default()
            },
            person,
        ));
    }
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
                common::get_stable_uuid(format!("person:{}", name.to_lowercase()));
            let mut person = db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                external_ids: db::ExternalIds {
                    tmdb: Some(member.id),
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = tmdb_image(member.profile_path.as_deref()) {
                person.set_image(db::ImageKind::Primary, url);
            }
            relations.push((
                db::MediaRelation {
                    left_media_id,
                    right_media_id: person_id,
                    weight: Some(i as i64),
                    role: Some(role),
                    ..Default::default()
                },
                person,
            ));
        }
    }
    relations
}

fn build_genre_relations(
    left_media_id: uuid::Uuid,
    genres: &[sdks::tmdb::Genre],
) -> Vec<(db::MediaRelation, db::Media)> {
    genres
        .iter()
        .map(|genre| {
            let name = &genre.name;
            let genre_id =
                common::get_stable_uuid(format!("genre:{}", name.to_lowercase()));
            (
                db::MediaRelation {
                    left_media_id,
                    right_media_id: genre_id,
                    ..Default::default()
                },
                db::Media {
                    id: genre_id,
                    title: name.clone(),
                    kind: db::MediaKind::Genre,
                    media_id: Some(format!("genre:{}", name.to_lowercase())),
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn is_404(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<ClientError>(),
        Some(ClientError::Http { status: 404, .. })
    )
}

async fn fetch_tmdb_meta(
    media: &db::Media,
    ctx: &AppContext,
) -> Result<Option<db::Media>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let api_key = config.get_tmdb_key().to_string();
    let metadata_country = config
        .metadata_country_code
        .as_deref()
        .map(db::normalize_country_alpha2)
        .unwrap_or_else(|| "US".to_string());

    let ids = &media.external_ids;

    let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
        .with_auth(sdks::BearerAuth { token: api_key });

    match media.kind {
        db::MediaKind::Movie => {
            // Use the TMDB ID directly if known; otherwise discover it via /find.
            let tmdb_movie_id: Option<i64> = if let Some(id) = ids.tmdb {
                Some(id)
            } else {
                let (external_id, external_source) = if let Some(ref imdb) = ids.imdb {
                    (imdb.clone(), "imdb_id")
                } else if let Some(tvdb) = ids.tvdb {
                    (tvdb.to_string(), "tvdb_id")
                } else {
                    return Ok(None);
                };
                client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id,
                            external_source: external_source.to_string(),
                        }
                        .with_cache(Duration::from_secs(360)),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.movie_results.into_iter().next().map(|m| m.id))
            };

            if let Some(tmdb_id) = tmdb_movie_id {
                let movie_details = client
                    .execute(
                        sdks::tmdb::MovieEndpoint::new(tmdb_id)
                            .with_cache(Duration::from_secs(360)),
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
                let rating =
                    movie_details
                        .release_dates
                        .as_ref()
                        .and_then(|release_dates| {
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
                        });
                let (certification, certification_age) = rating
                    .map(|(country, rating)| {
                        let label = tmdb_rating_label(&country, &rating);
                        let age = rating_age(&label, &country);
                        (Some(label), age)
                    })
                    .unwrap_or((None, None));
                let digital_released_at = movie_details
                    .release_dates
                    .as_ref()
                    .and_then(|rd| {
                        rd.results
                            .iter()
                            .flat_map(|country| country.release_dates.iter())
                            .filter(|e| e.release_type >= 4)
                            .filter_map(|e| e.release_date)
                            .min()
                    })
                    .map(|dt| dt.naive_utc());
                let mut patch = db::Media {
                    title: movie_details.title,
                    description: movie_details.overview,
                    released_at: movie_details
                        .release_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    digital_released_at,
                    runtime: movie_details.runtime.map(|r| r * 60),
                    rating_audience: movie_details.vote_average,
                    certification,
                    certification_age,
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(movie_details.poster_path.as_deref()) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(movie_details.backdrop_path.as_deref()) {
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                if let Some(url) = logo {
                    patch.set_image(db::ImageKind::Logo, url);
                }
                let mut relations = vec![];
                if let Some(genres) = &movie_details.genres {
                    relations.extend(build_genre_relations(media.id, genres));
                }
                if let Some(credits) = &movie_details.credits {
                    relations.extend(build_person_relations(media.id, credits));
                }
                if !relations.is_empty() {
                    patch.relations = Some(relations);
                }
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Series => {
            // Use the TMDB ID directly if known; otherwise discover it via /find.
            let tmdb_series_id: Option<i64> = if let Some(id) = ids.tmdb {
                Some(id)
            } else {
                let (external_id, external_source) = if let Some(ref imdb) = ids.imdb {
                    (imdb.clone(), "imdb_id")
                } else if let Some(tvdb) = ids.tvdb {
                    (tvdb.to_string(), "tvdb_id")
                } else {
                    return Ok(None);
                };
                client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id,
                            external_source: external_source.to_string(),
                        }
                        .with_cache(Duration::from_secs(360)),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.tv_results.into_iter().next().map(|s| s.id))
            };

            if let Some(tmdb_id) = tmdb_series_id {
                let tv_details = client
                    .execute(
                        sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                            .with_cache(Duration::from_secs(360)),
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
                let rating =
                    tv_details
                        .content_ratings
                        .as_ref()
                        .and_then(|content_ratings| {
                            select_rating(
                                &content_ratings.results,
                                &metadata_country,
                                |rating| rating.iso_3166_1.as_str(),
                                |rating| rating.rating.as_deref(),
                            )
                        });
                let (certification, certification_age) = rating
                    .map(|(country, rating)| {
                        let label = tmdb_rating_label(&country, &rating);
                        let age = rating_age(&label, &country);
                        (Some(label), age)
                    })
                    .unwrap_or((None, None));
                let mut patch = db::Media {
                    title: tv_details.name,
                    description: tv_details.overview,
                    released_at: tv_details
                        .first_air_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    rating_audience: tv_details.vote_average,
                    certification,
                    certification_age,
                    country,
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(tv_details.poster_path.as_deref()) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(tv_details.backdrop_path.as_deref()) {
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                if let Some(url) = logo {
                    patch.set_image(db::ImageKind::Logo, url);
                }
                let mut relations = vec![];
                if let Some(genres) = &tv_details.genres {
                    relations.extend(build_genre_relations(media.id, genres));
                }
                if let Some(credits) = &tv_details.credits {
                    relations.extend(build_person_relations(media.id, credits));
                }
                if let Some(creators) = &tv_details.created_by {
                    for (i, creator) in creators.iter().enumerate() {
                        let name = &creator.name;
                        let person_id = common::get_stable_uuid(format!(
                            "person:{}",
                            name.to_lowercase()
                        ));
                        let mut creator_media = db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            media_id: Some(format!("person:{}", name.to_lowercase())),
                            external_ids: db::ExternalIds {
                                tmdb: Some(creator.id as i64),
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        if let Some(url) = tmdb_image(creator.profile_path.as_deref()) {
                            creator_media.set_image(db::ImageKind::Primary, url);
                        }
                        relations.push((
                            db::MediaRelation {
                                left_media_id: media.id,
                                right_media_id: person_id,
                                weight: Some(i as i64),
                                role: Some(db::RelationRole::Creator),
                                ..Default::default()
                            },
                            creator_media,
                        ));
                    }
                }
                if !relations.is_empty() {
                    patch.relations = Some(relations);
                }
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Episode => {
            let mut series_tmdb_id = if let Some(sid) = media.grandparent_id {
                db::Media::get_by_id(&ctx.db, &sid)
                    .await?
                    .filter(|m| m.kind == db::MediaKind::Series)
                    .and_then(|m| m.external_ids.tmdb)
            } else {
                None
            };
            if series_tmdb_id.is_none() {
                if let Some(ref series_imdb) = media.grandparent_media_id {
                    let find = client
                        .execute(
                            sdks::tmdb::FindByIdEndpoint {
                                external_id: series_imdb.clone(),
                                external_source: "imdb_id".to_string(),
                            }
                            .with_cache(Duration::from_secs(360)),
                        )
                        .await
                        .ok();
                    series_tmdb_id = find
                        .and_then(|f| f.tv_results.into_iter().next().map(|r| r.id));
                }
            }
            let season_number = media.parent_idx;
            let episode_number = media.idx;
            if let (Some(tmdb_id), Some(s_n), Some(e_n)) =
                (series_tmdb_id, season_number, episode_number)
            {
                let ep_details = client
                    .execute(
                        sdks::tmdb::EpisodeEndpoint::new(tmdb_id, s_n, e_n)
                            .with_cache(Duration::from_secs(360)),
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
                let best_still = ep_details
                    .images
                    .as_ref()
                    .and_then(|imgs| {
                        imgs.stills
                            .iter()
                            .max_by(|a, b| {
                                a.vote_average
                                    .partial_cmp(&b.vote_average)
                                    .unwrap_or(std::cmp::Ordering::Equal)
                            })
                            .map(|e| e.file_path.clone())
                    })
                    .or_else(|| ep_details.still_path.clone());
                let still_url = tmdb_image(best_still.as_deref());
                let mut patch = db::Media {
                    title: ep_details.name,
                    description: ep_details.overview,
                    released_at: ep_details
                        .air_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    runtime: ep_details.runtime.map(|r| r * 60),
                    rating_audience: ep_details.vote_average,
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = still_url {
                    patch.set_image(db::ImageKind::Primary, url.clone());
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                let mut relations = vec![];
                if let Some(guest_stars) = &ep_details.guest_stars {
                    for (i, member) in guest_stars.iter().enumerate() {
                        let name = &member.name;
                        let person_id = common::get_stable_uuid(format!(
                            "person:{}",
                            name.to_lowercase()
                        ));
                        let mut person = db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            media_id: Some(format!("person:{}", name.to_lowercase())),
                            external_ids: db::ExternalIds {
                                tmdb: Some(member.id),
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        if let Some(url) = tmdb_image(member.profile_path.as_deref()) {
                            person.set_image(db::ImageKind::Primary, url);
                        }
                        relations.push((
                            db::MediaRelation {
                                left_media_id: media.id,
                                right_media_id: person_id,
                                weight: Some(i as i64),
                                role: Some(db::RelationRole::Actor),
                                character: member.character.clone(),
                                ..Default::default()
                            },
                            person,
                        ));
                    }
                }
                if let Some(credits) = &ep_details.credits {
                    let base_weight = relations.len() as i64;
                    let mut ep_relations = build_person_relations(media.id, credits);
                    for (rel, _) in &mut ep_relations {
                        if let Some(w) = rel.weight {
                            rel.weight = Some(base_weight + w);
                        } else {
                            rel.weight = Some(base_weight);
                        }
                    }
                    relations.extend(ep_relations);
                }
                if let Some(grandparent_id) = media.grandparent_id {
                    let series_genres = sqlx::query_as::<_, db::Media>(
                        "SELECT m.* FROM media m
                         JOIN media_relations r ON m.id = r.right_media_id
                         WHERE r.left_media_id = ? AND m.kind = 'genre'",
                    )
                    .bind(grandparent_id)
                    .fetch_all(&ctx.db)
                    .await
                    .unwrap_or_default();
                    for genre in series_genres {
                        relations.push((
                            db::MediaRelation {
                                left_media_id: media.id,
                                right_media_id: genre.id,
                                ..Default::default()
                            },
                            genre,
                        ));
                    }
                }
                if !relations.is_empty() {
                    patch.relations = Some(relations);
                }
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Season => {
            return fetch_tmdb_season_meta(media, ctx, &client).await;
        }
        db::MediaKind::Person => {
            let tmdb_id = if let Some(id) = media.external_ids.tmdb {
                id
            } else {
                // No stored TMDB ID — search by name to resolve it.
                let resp = client
                    .execute(sdks::tmdb::PersonSearchEndpoint {
                        query: media.title.clone(),
                    })
                    .await?;
                let Some(hit) = resp.results.into_iter().next() else {
                    return Ok(None);
                };
                hit.id
            };
            let details = client
                .execute(
                    sdks::tmdb::PersonDetailsEndpoint { person_id: tmdb_id }
                        .with_cache(Duration::from_secs(86400)),
                )
                .await?;
            let released_at = details.birthday.as_deref().and_then(|b| {
                chrono::NaiveDate::parse_from_str(b, "%Y-%m-%d")
                    .ok()
                    .and_then(|d| d.and_hms_opt(0, 0, 0))
            });
            let mut patch = db::Media {
                description: details.biography.filter(|b| !b.is_empty()),
                released_at,
                country: details.place_of_birth.filter(|p| !p.is_empty()),
                external_ids: db::ExternalIds {
                    tmdb: Some(tmdb_id),
                    imdb: details.imdb_id,
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = tmdb_image(details.profile_path.as_deref()) {
                patch.set_image(db::ImageKind::Primary, url);
            }
            return Ok(Some(patch));
        }
        _ => {}
    }

    Ok(None)
}

async fn fetch_tmdb_season_meta(
    media: &db::Media,
    ctx: &AppContext,
    client: &sdks::RestClient<sdks::BearerAuth>,
) -> Result<Option<db::Media>> {
    let series_tmdb_id = if let Some(sid) = media.grandparent_id.or(media.parent_id) {
        db::Media::get_by_id(&ctx.db, &sid)
            .await?
            .and_then(|m| m.external_ids.tmdb)
    } else {
        None
    };
    let (Some(tmdb_id), Some(season_idx)) = (series_tmdb_id, media.idx) else {
        return Ok(None);
    };
    let tv_details = client
        .execute(
            sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                .with_cache(Duration::from_secs(360)),
        )
        .await?;
    let season_data = tv_details
        .seasons
        .iter()
        .find(|s| s.season_number == season_idx);
    let Some(season) = season_data else {
        return Ok(None);
    };
    let mut patch = db::Media::default();
    if let Some(url) = tmdb_image(season.poster_path.as_deref()) {
        patch.set_image(db::ImageKind::Primary, url);
    }
    Ok(Some(patch))
}

// ---------------------------------------------------------------------------
// TMDB person search
// ---------------------------------------------------------------------------

async fn search_tmdb_person(
    query: &str,
    limit: usize,
    ctx: &AppContext,
) -> Result<Vec<db::Media>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let api_key = config.get_tmdb_key().to_string();

    let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
        .with_auth(sdks::BearerAuth { token: api_key });

    let resp = client
        .execute(sdks::tmdb::PersonSearchEndpoint {
            query: query.to_string(),
        })
        .await?;

    let media = resp
        .results
        .into_iter()
        .take(limit)
        .map(|p| {
            let media_id = format!("person:{}", p.name.to_lowercase());
            let id = common::get_stable_uuid(media_id.clone());
            let profile_url = p
                .profile_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("{}{}", TMDB_IMAGE_BASE, s));
            let mut media = db::Media {
                id,
                title: p.name,
                kind: db::MediaKind::Person,
                media_id: Some(media_id),
                ..Default::default()
            };
            if let Some(url) = profile_url {
                media.set_image(db::ImageKind::Primary, url);
            }
            ctx.store
                .insert(id.to_string(), media.clone(), Duration::from_secs(360));
            media
        })
        .collect();

    Ok(media)
}

// ---------------------------------------------------------------------------
// Remote images
// ---------------------------------------------------------------------------

async fn tmdb_remote_images(
    ctx: &AppContext,
    media: &db::Media,
) -> Result<Vec<api::RemoteImageInfo>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let api_key = config.get_tmdb_key().to_string();
    if api_key.is_empty() {
        return Ok(vec![]);
    }
    let client = sdks::RestClient::new("https://api.themoviedb.org/3/")?
        .with_auth(sdks::BearerAuth { token: api_key });

    let lookup_for_find = || -> Option<(String, &'static str)> {
        let ids = &media.external_ids;
        if let Some(tmdb_id) = ids.tmdb {
            return Some((tmdb_id.to_string(), "tmdb_id"));
        }
        if let Some(ref imdb) = ids.imdb {
            return Some((imdb.clone(), "imdb_id"));
        }
        if let Some(ref gp_media_id) = media.grandparent_media_id {
            return Some((gp_media_id.clone(), "imdb_id"));
        }
        if let Some(tvdb_id) = ids.tvdb {
            return Some((tvdb_id.to_string(), "tvdb_id"));
        }
        None
    };

    fn map_image(
        type_label: &str,
        entry: &sdks::tmdb::ImageEntry,
    ) -> api::RemoteImageInfo {
        let url = format!("{TMDB_IMAGE_BASE}{}", entry.file_path);
        let thumb = format!("https://image.tmdb.org/t/p/w300{}", entry.file_path);
        api::RemoteImageInfo {
            provider_name: Some("TheMovieDb".to_string()),
            url: Some(url),
            thumbnail_url: Some(thumb),
            type_: Some(type_label.to_string()),
            width: entry.width,
            height: entry.height,
        }
    }

    fn extend_from_images(
        out: &mut Vec<api::RemoteImageInfo>,
        images: &sdks::tmdb::Images,
    ) {
        out.extend(images.backdrops.iter().map(|e| map_image("Backdrop", e)));
        out.extend(images.posters.iter().map(|e| map_image("Primary", e)));
        out.extend(images.logos.iter().map(|e| map_image("Logo", e)));
        out.extend(images.stills.iter().map(|e| map_image("Backdrop", e)));
        out.extend(images.stills.iter().map(|e| map_image("Screenshot", e)));
        out.extend(images.stills.iter().map(|e| map_image("Thumb", e)));
    }

    let mut out = Vec::new();

    match media.kind {
        db::MediaKind::Movie => {
            let tmdb_id = if let Some(id) = media.external_ids.tmdb {
                Some(id)
            } else if let Some((external_id, external_source)) = lookup_for_find() {
                let find = client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id,
                            external_source: external_source.to_string(),
                        }
                        .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                find.movie_results.first().map(|m| m.id)
            } else {
                None
            };
            if let Some(tmdb_id) = tmdb_id {
                let movie = client
                    .execute(
                        sdks::tmdb::MovieEndpoint::new(tmdb_id)
                            .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                if let Some(images) = &movie.images {
                    extend_from_images(&mut out, images);
                }
                if out.iter().all(|i| i.type_.as_deref() != Some("Primary")) {
                    if let Some(p) = &movie.poster_path {
                        out.push(api::RemoteImageInfo {
                            provider_name: Some("TheMovieDb".to_string()),
                            url: Some(format!("{TMDB_IMAGE_BASE}{p}")),
                            thumbnail_url: Some(format!(
                                "https://image.tmdb.org/t/p/w300{p}"
                            )),
                            type_: Some("Primary".to_string()),
                            width: None,
                            height: None,
                        });
                    }
                }
                if out.iter().all(|i| i.type_.as_deref() != Some("Backdrop")) {
                    if let Some(b) = &movie.backdrop_path {
                        out.push(api::RemoteImageInfo {
                            provider_name: Some("TheMovieDb".to_string()),
                            url: Some(format!("{TMDB_IMAGE_BASE}{b}")),
                            thumbnail_url: Some(format!(
                                "https://image.tmdb.org/t/p/w300{b}"
                            )),
                            type_: Some("Backdrop".to_string()),
                            width: None,
                            height: None,
                        });
                    }
                }
            }
        }
        db::MediaKind::Series => {
            let tmdb_id = if let Some(id) = media.external_ids.tmdb {
                Some(id)
            } else if let Some((external_id, external_source)) = lookup_for_find() {
                let find = client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id,
                            external_source: external_source.to_string(),
                        }
                        .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                find.tv_results.first().map(|m| m.id)
            } else {
                None
            };
            if let Some(tmdb_id) = tmdb_id {
                let tv = client
                    .execute(
                        sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                            .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                if let Some(images) = &tv.images {
                    extend_from_images(&mut out, images);
                }
            }
        }
        db::MediaKind::Episode => {
            let mut series_tmdb_id = if let Some(sid) = media.grandparent_id {
                db::Media::get_by_id(&ctx.db, &sid)
                    .await?
                    .and_then(|m| m.external_ids.tmdb)
            } else {
                None
            };
            if series_tmdb_id.is_none() {
                if let Some(ref series_imdb) = media.grandparent_media_id {
                    let find = client
                        .execute(
                            sdks::tmdb::FindByIdEndpoint {
                                external_id: series_imdb.clone(),
                                external_source: "imdb_id".to_string(),
                            }
                            .with_cache(Duration::from_secs(360)),
                        )
                        .await
                        .ok();
                    series_tmdb_id = find
                        .and_then(|f| f.tv_results.into_iter().next().map(|r| r.id));
                }
            }
            if let (Some(tmdb_id), Some(s_n), Some(e_n)) =
                (series_tmdb_id, media.parent_idx, media.idx)
            {
                let ep = client
                    .execute(
                        sdks::tmdb::EpisodeEndpoint::new(tmdb_id, s_n, e_n)
                            .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                if let Some(images) = &ep.images {
                    extend_from_images(&mut out, images);
                }
                if out.iter().all(|i| i.type_.as_deref() != Some("Thumb")) {
                    if let Some(p) = &ep.still_path {
                        let url = format!("{TMDB_IMAGE_BASE}{p}");
                        let thumb = format!("https://image.tmdb.org/t/p/w300{p}");
                        out.push(api::RemoteImageInfo {
                            provider_name: Some("TheMovieDb".to_string()),
                            url: Some(url.clone()),
                            thumbnail_url: Some(thumb.clone()),
                            type_: Some("Backdrop".to_string()),
                            width: None,
                            height: None,
                        });
                        out.push(api::RemoteImageInfo {
                            provider_name: Some("TheMovieDb".to_string()),
                            url: Some(url.clone()),
                            thumbnail_url: Some(thumb.clone()),
                            type_: Some("Screenshot".to_string()),
                            width: None,
                            height: None,
                        });
                        out.push(api::RemoteImageInfo {
                            provider_name: Some("TheMovieDb".to_string()),
                            url: Some(url),
                            thumbnail_url: Some(thumb),
                            type_: Some("Thumb".to_string()),
                            width: None,
                            height: None,
                        });
                    }
                }
            }
        }
        _ => {}
    }

    Ok(out)
}

/// Resolve an IMDB ID from already-known external IDs without doing a title search.
///
/// Resolution order: direct IMDB → TMDB lookup → TVDB lookup via FindById.
pub(crate) async fn resolve_imdb_from_ids<A: sdks::Auth + Clone>(
    ids: &db::ExternalIds,
    is_tv: bool,
    client: &sdks::RestClient<A>,
) -> Option<String> {
    if let Some(ref imdb) = ids.imdb {
        return Some(imdb.clone());
    }

    if let Some(tmdb_id) = ids.tmdb {
        if is_tv {
            match client
                .execute(
                    sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                        .with_cache(Duration::from_secs(86400)),
                )
                .await
            {
                Ok(series) => {
                    if let Some(imdb) = series.external_ids.and_then(|e| e.imdb_id) {
                        return Some(imdb);
                    }
                    debug!(tmdb_id, "TMDB series has no imdb_id in external_ids");
                }
                Err(e) => warn!(tmdb_id, error = %e, "TMDB series lookup failed"),
            }
        } else {
            match client
                .execute(
                    sdks::tmdb::MovieEndpoint::new(tmdb_id)
                        .with_cache(Duration::from_secs(86400)),
                )
                .await
            {
                Ok(movie) => {
                    if let Some(imdb) = movie.imdb_id {
                        return Some(imdb);
                    }
                    debug!(tmdb_id, "TMDB movie has no imdb_id");
                }
                Err(e) => warn!(tmdb_id, error = %e, "TMDB movie lookup failed"),
            }
        }
    }

    if let Some(tvdb_id) = ids.tvdb {
        let find_resp = client
            .execute(
                sdks::tmdb::FindByIdEndpoint {
                    external_id: tvdb_id.to_string(),
                    external_source: "tvdb_id".to_string(),
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        return if is_tv {
            find_resp
                .tv_results
                .into_iter()
                .next()
                .and_then(|s| s.external_ids)
                .and_then(|e| e.imdb_id)
        } else {
            find_resp
                .movie_results
                .into_iter()
                .next()
                .and_then(|m| m.imdb_id)
        };
    }

    None
}
