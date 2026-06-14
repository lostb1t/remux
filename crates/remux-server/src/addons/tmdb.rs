use anyhow::Result;
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    MediaKind, MetaAddon, ResourceType, SearchAddon, TreeAddon,
};
use crate::{
    AppContext, api, common, db, sdks,
    sdks::{CachedEndpoint, ClientError},
};

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
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let addon = Arc::new(TmdbAddon {});
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            meta: Some(addon.clone()),
            search: Some(addon.clone()),
            tree: Some(addon),
            ..Default::default()
        })
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
}

#[async_trait]
impl MetaAddon for TmdbAddon {
    async fn supports(&self, media: &db::Media) -> bool {
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
        config: &crate::api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        match fetch_tmdb_meta(media, ctx, config).await {
            Err(e) if is_404(&e) => Ok(None),
            other => other,
        }
    }

    async fn images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        tmdb_remote_images(ctx, media).await
    }
}

#[async_trait]
impl SearchAddon for TmdbAddon {
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
}

// ---------------------------------------------------------------------------
// TMDB SDK type → db::Media conversions
// ---------------------------------------------------------------------------

impl From<&sdks::tmdb::Season> for db::Media {
    fn from(s: &sdks::tmdb::Season) -> Self {
        let air_date = s
            .air_date
            .and_then(|d| d.and_hms_opt(0, 0, 0));
        let mut media = db::Media {
            kind: db::MediaKind::Season,
            title: format!("Season {}", s.season_number),
            description: s
                .overview
                .clone()
                .filter(|o| !o.is_empty()),
            idx: Some(s.season_number),
            external_ids: db::ExternalIds {
                tmdb: Some(s.id),
                ..Default::default()
            },
            released_at: air_date,
            digital_released_at: air_date,
            ..Default::default()
        };
        if let Some(url) = tmdb_image(
            s.poster_path
                .as_deref(),
        ) {
            media.set_image(db::ImageKind::Primary, url);
        }
        media
    }
}

impl From<&sdks::tmdb::Episode> for db::Media {
    fn from(ep: &sdks::tmdb::Episode) -> Self {
        let mut media = db::Media {
            kind: db::MediaKind::Episode,
            title: format!("S{}E{} - {}", ep.season_number, ep.episode_number, ep.name),
            description: ep
                .overview
                .clone()
                .filter(|o| !o.is_empty()),
            idx: Some(ep.episode_number),
            parent_idx: Some(ep.season_number),
            external_ids: db::ExternalIds {
                tmdb: Some(ep.id),
                ..Default::default()
            },
            released_at: ep
                .air_date
                .and_then(|d| d.and_hms_opt(0, 0, 0)),
            refreshed_at: Some(chrono::Utc::now().naive_utc()),
            ..Default::default()
        };
        if let Some(url) = tmdb_image(
            ep.still_path
                .as_deref(),
        ) {
            media.set_image(db::ImageKind::Primary, url);
        }
        media
    }
}

// ---------------------------------------------------------------------------
// TMDB tree (seasons + episodes)
// ---------------------------------------------------------------------------

#[async_trait]
impl TreeAddon for TmdbAddon {
    fn supports(&self, root: &db::Media) -> bool {
        matches!(root.kind, db::MediaKind::Series | db::MediaKind::Season)
    }

    async fn get_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        match root.kind {
            db::MediaKind::Series => tmdb_series_seasons(root, ctx).await,
            db::MediaKind::Season => tmdb_season_episodes(root, ctx).await,
            _ => Ok(None),
        }
    }
}

fn tmdb_client(api_key: &str) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    Ok(
        sdks::RestClient::new("https://api.themoviedb.org/3/")?.with_auth(
            sdks::BearerAuth {
                token: api_key.to_string(),
            },
        ),
    )
}

async fn tmdb_client_from_ctx(
    ctx: &AppContext,
) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    tmdb_client(config.get_tmdb_key())
}

async fn tmdb_series_seasons(
    series: &db::Media,
    ctx: &AppContext,
) -> Result<Option<Vec<db::Media>>> {
    let Some(tmdb_id) = series
        .external_ids
        .tmdb
    else {
        return Ok(None);
    };
    let client = tmdb_client_from_ctx(ctx).await?;
    let tv = client
        .execute(
            sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                .with_cache(Duration::from_secs(360)),
        )
        .await?;

    let series_imdb: String = tv
        .external_ids
        .as_ref()
        .and_then(|e| {
            e.imdb_id
                .clone()
        })
        .or_else(|| {
            series
                .external_ids
                .imdb
                .clone()
        })
        .unwrap_or_else(|| format!("tmdb:{}", tmdb_id));

    let seasons: Vec<db::Media> = tv
        .seasons
        .iter()
        .map(|s| {
            let mut stub = db::Media::from(s);
            stub.id = common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:{}", series_imdb, s.season_number),
            );
            stub.parent_id = Some(series.id);
            stub.grandparent_id = Some(series.id);
            stub.external_ids
                .series_imdb = Some(series_imdb.clone());
            stub.external_ids
                .series_tmdb = Some(tmdb_id);
            stub
        })
        .collect();

    if seasons.is_empty() {
        Ok(None)
    } else {
        Ok(Some(seasons))
    }
}

async fn tmdb_season_episodes(
    season: &db::Media,
    ctx: &AppContext,
) -> Result<Option<Vec<db::Media>>> {
    let (Some(series_tmdb_id), Some(season_number), Some(ref series_imdb)) = (
        season
            .external_ids
            .series_tmdb,
        season.idx,
        season
            .external_ids
            .series_imdb
            .clone(),
    ) else {
        return Ok(None);
    };
    let client = tmdb_client_from_ctx(ctx).await?;
    let season_details = client
        .execute(
            sdks::tmdb::SeasonEndpoint {
                series_id: series_tmdb_id,
                season_number: season_number as i64,
                language: None,
                append_to_response: None,
            }
            .with_cache(Duration::from_secs(360)),
        )
        .await?;

    let episodes: Vec<db::Media> = season_details
        .episodes
        .unwrap_or_default()
        .into_iter()
        .map(|ep| {
            let mut stub = db::Media::from(&ep);
            stub.id = common::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:{}:{}", series_imdb, ep.season_number, ep.episode_number),
            );
            stub.parent_id = Some(common::stable_media_uuid(
                &db::MediaKind::Season,
                &format!("{}:{}", series_imdb, ep.season_number),
            ));
            stub.grandparent_id = season.parent_id;
            stub.external_ids
                .series_imdb = Some(series_imdb.clone());
            stub.external_ids
                .series_tmdb = Some(series_tmdb_id);
            stub
        })
        .collect();

    if episodes.is_empty() {
        Ok(None)
    } else {
        Ok(Some(episodes))
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
        .or_else(|| {
            ratings
                .iter()
                .find_map(valid)
        })
}

fn tmdb_rating_label(country: &str, rating: &str) -> String {
    if country.eq_ignore_ascii_case("US") {
        rating.to_string()
    } else if country.eq_ignore_ascii_case("DE")
        && !rating
            .to_uppercase()
            .starts_with("FSK")
    {
        format!("FSK-{rating}")
    } else {
        rating.to_string()
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
    for (i, member) in credits
        .cast
        .iter()
        .enumerate()
    {
        let name = &member.name;
        let person_id = common::stable_media_uuid(
            &db::MediaKind::Person,
            &member
                .id
                .to_string(),
        );
        let mut person = db::Media {
            id: person_id,
            title: name.clone(),
            kind: db::MediaKind::Person,
            external_ids: db::ExternalIds {
                tmdb: Some(member.id),
                ..Default::default()
            },
            ..Default::default()
        };
        if let Some(url) = tmdb_image(
            member
                .profile_path
                .as_deref(),
        ) {
            person.set_image(db::ImageKind::Primary, url);
        }
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(i as i64),
                role: Some(db::RelationRole::Actor),
                character: member
                    .character
                    .clone(),
                ..Default::default()
            },
            person,
        ));
    }
    for (i, member) in credits
        .crew
        .iter()
        .enumerate()
    {
        let role = match member
            .job
            .as_str()
        {
            "Director" => Some(db::RelationRole::Director),
            "Writer" | "Screenplay" | "Author" => Some(db::RelationRole::Writer),
            "Producer" | "Executive Producer" | "Co-Producer" => {
                Some(db::RelationRole::Producer)
            }
            _ => None,
        };
        if let Some(role) = role {
            let name = &member.name;
            let person_id = common::stable_media_uuid(
                &db::MediaKind::Person,
                &member
                    .id
                    .to_string(),
            );
            let mut person = db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                external_ids: db::ExternalIds {
                    tmdb: Some(member.id),
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = tmdb_image(
                member
                    .profile_path
                    .as_deref(),
            ) {
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
                common::stable_media_uuid(&db::MediaKind::Genre, &name.to_lowercase());
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
    config: &crate::api::ServerConfiguration,
) -> Result<Option<db::Media>> {
    let metadata_country = config
        .metadata_country_code
        .as_deref()
        .map(db::normalize_country_alpha2)
        .unwrap_or_else(|| "US".to_string());

    let ids = &media.external_ids;

    let client = tmdb_client(config.get_tmdb_key())?;

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
                    .and_then(|r| {
                        r.movie_results
                            .into_iter()
                            .next()
                            .map(|m| m.id)
                    })
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
                    imdb: movie_details
                        .imdb_id
                        .clone()
                        .or(ids
                            .imdb
                            .clone()),
                    tvdb: ids.tvdb,
                    ..Default::default()
                };
                let logo = movie_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_logo())
                    .and_then(|p| tmdb_image(Some(p)));
                let thumb = movie_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_thumb())
                    .and_then(|p| tmdb_image(Some(p)));
                let rating = movie_details
                    .release_dates
                    .as_ref()
                    .and_then(|release_dates| {
                        let releases = release_dates
                            .results
                            .iter()
                            .flat_map(|country| {
                                country
                                    .release_dates
                                    .iter()
                                    .map(|release| {
                                        (
                                            country
                                                .iso_3166_1
                                                .as_str(),
                                            release
                                                .certification
                                                .as_deref(),
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
                            .flat_map(|country| {
                                country
                                    .release_dates
                                    .iter()
                            })
                            .filter(|e| e.release_type >= 4)
                            .filter_map(|e| e.release_date)
                            .min()
                    })
                    .map(|dt| dt.naive_utc());
                let external_ratings = db::ExternalRatings {
                    tmdb: movie_details
                        .vote_average
                        .map(|score| db::Rating {
                            score,
                            vote_count: movie_details
                                .vote_count
                                .map(|v| v as u32),
                        }),
                };
                let mut patch = db::Media {
                    title: movie_details.title,
                    description: movie_details.overview,
                    released_at: movie_details
                        .release_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    digital_released_at,
                    runtime: movie_details
                        .runtime
                        .map(|r| r * 60),
                    rating_audience: external_ratings.audience_rating(),
                    external_ratings: Some(external_ratings),
                    certification,
                    certification_age,
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(
                    movie_details
                        .poster_path
                        .as_deref(),
                ) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(
                    movie_details
                        .backdrop_path
                        .as_deref(),
                ) {
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                if let Some(url) = logo {
                    patch.set_image(db::ImageKind::Logo, url);
                }
                if let Some(url) = thumb {
                    patch.set_image(db::ImageKind::Thumb, url);
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
                    .and_then(|r| {
                        r.tv_results
                            .into_iter()
                            .next()
                            .map(|s| s.id)
                    })
            };

            if let Some(tmdb_id) = tmdb_series_id {
                let tv_details = client
                    .execute(
                        sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                            .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                let tmdb_ext = tv_details
                    .external_ids
                    .as_ref();
                let external_ids = db::ExternalIds {
                    tmdb: Some(tv_details.id),
                    imdb: tmdb_ext.and_then(|e| {
                        e.imdb_id
                            .clone()
                    }),
                    tvdb: tmdb_ext.and_then(|e| e.tvdb_id),
                    ..Default::default()
                };
                let country = tv_details
                    .origin_country
                    .into_iter()
                    .next();
                let logo = tv_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_logo())
                    .and_then(|p| tmdb_image(Some(p)));
                let thumb = tv_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_thumb())
                    .and_then(|p| tmdb_image(Some(p)));
                let rating = tv_details
                    .content_ratings
                    .as_ref()
                    .and_then(|content_ratings| {
                        select_rating(
                            &content_ratings.results,
                            &metadata_country,
                            |rating| {
                                rating
                                    .iso_3166_1
                                    .as_str()
                            },
                            |rating| {
                                rating
                                    .rating
                                    .as_deref()
                            },
                        )
                    });
                let (certification, certification_age) = rating
                    .map(|(country, rating)| {
                        let label = tmdb_rating_label(&country, &rating);
                        let age = rating_age(&label, &country);
                        (Some(label), age)
                    })
                    .unwrap_or((None, None));
                let external_ratings = db::ExternalRatings {
                    tmdb: tv_details
                        .vote_average
                        .map(|score| db::Rating {
                            score,
                            vote_count: Some(tv_details.vote_count as u32),
                        }),
                };
                let mut patch = db::Media {
                    title: tv_details.name,
                    description: tv_details.overview,
                    released_at: tv_details
                        .first_air_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    rating_audience: external_ratings.audience_rating(),
                    external_ratings: Some(external_ratings),
                    certification,
                    certification_age,
                    country,
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(
                    tv_details
                        .poster_path
                        .as_deref(),
                ) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(
                    tv_details
                        .backdrop_path
                        .as_deref(),
                ) {
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                if let Some(url) = logo {
                    patch.set_image(db::ImageKind::Logo, url);
                }
                if let Some(url) = thumb {
                    patch.set_image(db::ImageKind::Thumb, url);
                }
                let mut relations = vec![];
                if let Some(genres) = &tv_details.genres {
                    relations.extend(build_genre_relations(media.id, genres));
                }
                if let Some(credits) = &tv_details.credits {
                    relations.extend(build_person_relations(media.id, credits));
                }
                if let Some(creators) = &tv_details.created_by {
                    for (i, creator) in creators
                        .iter()
                        .enumerate()
                    {
                        let name = &creator.name;
                        let tmdb_id = creator.id as i64;
                        let person_id = common::stable_media_uuid(
                            &db::MediaKind::Person,
                            &tmdb_id.to_string(),
                        );
                        let mut creator_media = db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            external_ids: db::ExternalIds {
                                tmdb: Some(tmdb_id),
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        if let Some(url) = tmdb_image(
                            creator
                                .profile_path
                                .as_deref(),
                        ) {
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
            let mut series_tmdb_id = if let Some(tmdb) = media
                .grandparent
                .as_ref()
                .and_then(|g| {
                    g.external_ids
                        .tmdb
                }) {
                Some(tmdb)
            } else if let Some(sid) = media.grandparent_id {
                db::Media::get_by_id(&ctx.db, &sid)
                    .await?
                    .filter(|m| m.kind == db::MediaKind::Series)
                    .and_then(|m| {
                        m.external_ids
                            .tmdb
                    })
            } else {
                None
            };
            if series_tmdb_id.is_none() {
                if let Some(ref series_imdb) = media
                    .external_ids
                    .series_imdb
                {
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
                    series_tmdb_id = find.and_then(|f| {
                        f.tv_results
                            .into_iter()
                            .next()
                            .map(|r| r.id)
                    });
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
                let tmdb_ext = ep_details
                    .external_ids
                    .as_ref();
                let external_ids = db::ExternalIds {
                    tmdb: Some(ep_details.id),
                    imdb: tmdb_ext.and_then(|e| {
                        e.imdb_id
                            .clone()
                    }),
                    tvdb: tmdb_ext.and_then(|e| e.tvdb_id),
                    ..Default::default()
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
                            .map(|e| {
                                e.file_path
                                    .clone()
                            })
                    })
                    .or_else(|| {
                        ep_details
                            .still_path
                            .clone()
                    });
                let still_url = tmdb_image(best_still.as_deref());
                let external_ratings = db::ExternalRatings {
                    tmdb: ep_details
                        .vote_average
                        .map(|score| db::Rating {
                            score,
                            vote_count: ep_details
                                .vote_count
                                .map(|v| v as u32),
                        }),
                };
                let mut patch = db::Media {
                    title: ep_details.name,
                    description: ep_details.overview,
                    released_at: ep_details
                        .air_date
                        .and_then(|d| d.and_hms_opt(0, 0, 0)),
                    runtime: ep_details
                        .runtime
                        .map(|r| r * 60),
                    rating_audience: external_ratings.audience_rating(),
                    external_ratings: Some(external_ratings),
                    external_ids: external_ids,
                    ..Default::default()
                };
                if let Some(url) = still_url {
                    patch.set_image(db::ImageKind::Primary, url.clone());
                    patch.set_image(db::ImageKind::Backdrop, url);
                }
                let mut relations = vec![];
                if let Some(guest_stars) = &ep_details.guest_stars {
                    for (i, member) in guest_stars
                        .iter()
                        .enumerate()
                    {
                        let name = &member.name;
                        let person_id = common::stable_media_uuid(
                            &db::MediaKind::Person,
                            &member
                                .id
                                .to_string(),
                        );
                        let mut person = db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            external_ids: db::ExternalIds {
                                tmdb: Some(member.id),
                                ..Default::default()
                            },
                            ..Default::default()
                        };
                        if let Some(url) = tmdb_image(
                            member
                                .profile_path
                                .as_deref(),
                        ) {
                            person.set_image(db::ImageKind::Primary, url);
                        }
                        relations.push((
                            db::MediaRelation {
                                left_media_id: media.id,
                                right_media_id: person_id,
                                weight: Some(i as i64),
                                role: Some(db::RelationRole::Actor),
                                character: member
                                    .character
                                    .clone(),
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
                let series_genres: Vec<db::Media> = if let Some(genres) = media
                    .grandparent
                    .as_ref()
                    .and_then(|g| {
                        g.relations
                            .as_ref()
                    })
                    .map(|rels| {
                        rels.iter()
                            .filter(|(_, m)| m.kind == db::MediaKind::Genre)
                            .map(|(_, m)| m.clone())
                            .collect::<Vec<_>>()
                    })
                    .filter(|v: &Vec<_>| !v.is_empty())
                {
                    genres
                } else if let Some(grandparent_id) = media.grandparent_id {
                    sqlx::query_as::<_, db::Media>(
                        "SELECT m.* FROM media m
                         JOIN media_relations r ON m.id = r.right_media_id
                         WHERE r.left_media_id = ? AND m.kind = 'genre'",
                    )
                    .bind(grandparent_id)
                    .fetch_all(&ctx.db)
                    .await
                    .unwrap_or_default()
                } else {
                    vec![]
                };
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
            let tmdb_id = if let Some(id) = media
                .external_ids
                .tmdb
            {
                id
            } else {
                // No stored TMDB ID — search by name to resolve it.
                let resp = client
                    .execute(sdks::tmdb::PersonSearchEndpoint {
                        query: media
                            .title
                            .clone(),
                    })
                    .await?;
                let Some(hit) = resp
                    .results
                    .into_iter()
                    .next()
                else {
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
            let released_at = details
                .birthday
                .as_deref()
                .and_then(|b| {
                    chrono::NaiveDate::parse_from_str(b, "%Y-%m-%d")
                        .ok()
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                });
            let mut patch = db::Media {
                description: details
                    .biography
                    .filter(|b| !b.is_empty()),
                released_at,
                country: details
                    .place_of_birth
                    .filter(|p| !p.is_empty()),
                external_ids: db::ExternalIds {
                    tmdb: Some(tmdb_id),
                    imdb: details.imdb_id,
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = tmdb_image(
                details
                    .profile_path
                    .as_deref(),
            ) {
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
    // Try to get the series TMDB ID — prefer in-memory grandparent, fall back to DB.
    let series_tmdb_id = if let Some(tmdb) = media
        .grandparent
        .as_ref()
        .and_then(|g| {
            g.external_ids
                .tmdb
        }) {
        Some(tmdb)
    } else if let Some(sid) = media
        .grandparent_id
        .or(media.parent_id)
    {
        db::Media::get_by_id(&ctx.db, &sid)
            .await?
            .and_then(|m| {
                m.external_ids
                    .tmdb
            })
    } else {
        None
    };

    // On first import the series may not yet be flushed to DB with its TMDB ID.
    // Fall back to resolving via the season's series_imdb field.
    let series_tmdb_id = if series_tmdb_id.is_none() {
        if let Some(ref imdb) = media
            .external_ids
            .series_imdb
        {
            client
                .execute(
                    sdks::tmdb::FindByIdEndpoint {
                        external_id: imdb.clone(),
                        external_source: "imdb_id".to_string(),
                    }
                    .with_cache(Duration::from_secs(86400)),
                )
                .await
                .ok()
                .and_then(|r| {
                    r.tv_results
                        .into_iter()
                        .next()
                        .map(|s| s.id)
                })
        } else {
            None
        }
    } else {
        series_tmdb_id
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
    if let Some(url) = tmdb_image(
        season
            .poster_path
            .as_deref(),
    ) {
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
    let client = tmdb_client(config.get_tmdb_key())?;

    let resp = match client
        .execute(sdks::tmdb::PersonSearchEndpoint {
            query: query.to_string(),
        })
        .await
    {
        Ok(r) => r,
        // TMDB occasionally returns a non-JSON body (HTML rate-limit/CDN page) with
        // status 200 for person searches. Treat as zero results rather than surfacing
        // a spurious WARN on every general search that includes the Person type.
        Err(ClientError::Json { ref source, .. }) => {
            debug!(error = %source, query, "tmdb person search returned non-JSON body");
            return Ok(vec![]);
        }
        Err(e) => return Err(e.into()),
    };

    let media = resp
        .results
        .into_iter()
        .take(limit)
        .map(|p| {
            let id = common::stable_media_uuid(
                &db::MediaKind::Person,
                &p.id
                    .to_string(),
            );
            let profile_url = p
                .profile_path
                .as_deref()
                .filter(|s| !s.is_empty())
                .map(|s| format!("{}{}", TMDB_IMAGE_BASE, s));
            let mut media = db::Media {
                id,
                title: p.name,
                kind: db::MediaKind::Person,
                external_ids: db::ExternalIds {
                    tmdb: Some(p.id),
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = profile_url {
                media.set_image(db::ImageKind::Primary, url);
            }
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
    let api_key = config.get_tmdb_key();
    if api_key.is_empty() {
        return Ok(vec![]);
    }
    let client = tmdb_client(api_key)?;

    let lookup_for_find = || -> Option<(String, &'static str)> {
        let ids = &media.external_ids;
        if let Some(tmdb_id) = ids.tmdb {
            return Some((tmdb_id.to_string(), "tmdb_id"));
        }
        if let Some(ref imdb) = ids.imdb {
            return Some((imdb.clone(), "imdb_id"));
        }
        if let Some(ref series_imdb) = ids.series_imdb {
            return Some((series_imdb.clone(), "imdb_id"));
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
        out.extend(
            images
                .backdrops
                .iter()
                .map(|e| map_image("Backdrop", e)),
        );
        out.extend(
            images
                .posters
                .iter()
                .map(|e| map_image("Primary", e)),
        );
        out.extend(
            images
                .logos
                .iter()
                .map(|e| map_image("Logo", e)),
        );
        out.extend(
            images
                .stills
                .iter()
                .map(|e| map_image("Backdrop", e)),
        );
        out.extend(
            images
                .stills
                .iter()
                .map(|e| map_image("Screenshot", e)),
        );
        out.extend(
            images
                .stills
                .iter()
                .map(|e| map_image("Thumb", e)),
        );
    }

    let mut out = Vec::new();

    match media.kind {
        db::MediaKind::Movie => {
            let tmdb_id = if let Some(id) = media
                .external_ids
                .tmdb
            {
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
                find.movie_results
                    .first()
                    .map(|m| m.id)
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
                if out
                    .iter()
                    .all(|i| {
                        i.type_
                            .as_deref()
                            != Some("Primary")
                    })
                {
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
                if out
                    .iter()
                    .all(|i| {
                        i.type_
                            .as_deref()
                            != Some("Backdrop")
                    })
                {
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
            let tmdb_id = if let Some(id) = media
                .external_ids
                .tmdb
            {
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
                find.tv_results
                    .first()
                    .map(|m| m.id)
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
                    .and_then(|m| {
                        m.external_ids
                            .tmdb
                    })
            } else {
                None
            };
            if series_tmdb_id.is_none() {
                if let Some(ref series_imdb) = media
                    .external_ids
                    .series_imdb
                {
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
                    series_tmdb_id = find.and_then(|f| {
                        f.tv_results
                            .into_iter()
                            .next()
                            .map(|r| r.id)
                    });
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
                if out
                    .iter()
                    .all(|i| {
                        i.type_
                            .as_deref()
                            != Some("Thumb")
                    })
                {
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
                    if let Some(imdb) = series
                        .external_ids
                        .and_then(|e| e.imdb_id)
                    {
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
