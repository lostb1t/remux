use anyhow::Result;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use std::{pin::Pin, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    CatalogAddon, CatalogInfo, MediaKind, MetaAddon, MetricSnapshot, MetricValue,
    MetricsAddon, ResourceType, SearchAddon, TreeAddon,
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
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Meta),
                AddonMetadata::simple_resource(ResourceType::Search),
                AddonMetadata::simple_resource(ResourceType::Catalog),
                AddonMetadata::simple_resource(ResourceType::Metrics),
            ],
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
        let addon = Arc::new(TmdbAddon {
            popularity_max: Mutex::new(None),
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            meta: Some(addon.clone()),
            search: Some(addon.clone()),
            tree: Some(addon.clone()),
            catalog: Some(addon.clone()),
            metrics: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TmdbPreset))
}

pub struct TmdbAddon {
    // (fetched_at, movie_max, tv_max) — refreshed every hour from discover page 1
    popularity_max: Mutex<Option<(chrono::DateTime<chrono::Utc>, f64, f64)>>,
}

impl TmdbAddon {
    async fn popularity_max(&self, ctx: &AppContext) -> (f64, f64) {
        let now = chrono::Utc::now();
        let mut cache = self
            .popularity_max
            .lock()
            .await;
        if cache
            .as_ref()
            .map_or(true, |(t, _, _)| (now - *t).num_minutes() >= 60)
        {
            let Ok(config) = crate::db::Settings::get_config(&ctx.db).await else {
                return (1_000.0, 1_000.0);
            };
            let Ok(client) = tmdb_client(
                config.get_tmdb_key(),
                &ctx.config
                    .tmdb_base_url,
            ) else {
                return (1_000.0, 1_000.0);
            };
            let q = sdks::tmdb::DiscoverQuery {
                sort_by: Some("popularity.desc".into()),
                page: Some(1),
                ..Default::default()
            };
            let movie_max = client
                .execute(sdks::tmdb::DiscoverMovieEndpoint { query: q.clone() })
                .await
                .ok()
                .and_then(|r| {
                    r.results
                        .into_iter()
                        .next()
                })
                .and_then(|m| m.popularity)
                .unwrap_or(1_000.0);
            let tv_max = client
                .execute(sdks::tmdb::DiscoverTvEndpoint { query: q })
                .await
                .ok()
                .and_then(|r| {
                    r.results
                        .into_iter()
                        .next()
                })
                .and_then(|s| s.popularity)
                .unwrap_or(1_000.0);
            *cache = Some((now, movie_max, tv_max));
        }
        let (_, movie_max, tv_max) = cache.unwrap();
        (movie_max, tv_max)
    }
}

fn tmdb_image(path: Option<&str>, kind: db::ImageKind) -> Option<String> {
    let size = match kind {
        db::ImageKind::Backdrop => "w1280",
        db::ImageKind::Logo => "w500",
        db::ImageKind::Primary | db::ImageKind::Thumb => "w780",
    };
    path.filter(|p| !p.is_empty())
        .map(|p| format!("https://image.tmdb.org/t/p/{}{}", size, p))
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
        matches!(
            kind,
            db::MediaKind::Person | db::MediaKind::Movie | db::MediaKind::Series
        )
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        match kind {
            db::MediaKind::Person => {
                Ok(Some(search_tmdb_person(query, limit, ctx).await?))
            }
            db::MediaKind::Movie => {
                Ok(Some(search_tmdb_movie(query, limit, ctx).await?))
            }
            db::MediaKind::Series => {
                Ok(Some(search_tmdb_series(query, limit, ctx).await?))
            }
            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// CatalogAddon
// ---------------------------------------------------------------------------

struct CatalogDef {
    id: &'static str,
    name: &'static str,
    kind: db::MediaKind,
    collection_kind: db::CollectionMediaKind,
}

const TMDB_CATALOGS: &[CatalogDef] = &[
    CatalogDef {
        id: "popular_movies",
        name: "Popular Movies",
        kind: db::MediaKind::Movie,
        collection_kind: db::CollectionMediaKind::Movie,
    },
    CatalogDef {
        id: "popular_tv",
        name: "Popular TV Shows",
        kind: db::MediaKind::Series,
        collection_kind: db::CollectionMediaKind::Series,
    },
    CatalogDef {
        id: "top_rated_movies",
        name: "Top Rated Movies",
        kind: db::MediaKind::Movie,
        collection_kind: db::CollectionMediaKind::Movie,
    },
    CatalogDef {
        id: "top_rated_tv",
        name: "Top Rated TV Shows",
        kind: db::MediaKind::Series,
        collection_kind: db::CollectionMediaKind::Series,
    },
    CatalogDef {
        id: "trending_movies_week",
        name: "Trending Movies This Week",
        kind: db::MediaKind::Movie,
        collection_kind: db::CollectionMediaKind::Movie,
    },
    CatalogDef {
        id: "trending_tv_week",
        name: "Trending TV This Week",
        kind: db::MediaKind::Series,
        collection_kind: db::CollectionMediaKind::Series,
    },
];

#[async_trait]
impl CatalogAddon for TmdbAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(TMDB_CATALOGS
            .iter()
            .map(|c| CatalogInfo {
                media_kind: Some(
                    c.kind
                        .clone(),
                ),
                collection_media_kind: Some(
                    c.collection_kind
                        .clone(),
                ),
                default_enabled: false,
                default_max_items: Some(100),
                ..CatalogInfo::new(c.id, c.name)
            })
            .collect())
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let client = tmdb_client(
            config.get_tmdb_key(),
            &ctx.config
                .tmdb_base_url,
        )?;

        let stream: Pin<Box<dyn Stream<Item = db::Media> + Send>> = match local_id {
            "popular_movies" => Box::pin(with_imdb_resolved(
                discover_movie_stream(
                    client.clone(),
                    sdks::tmdb::DiscoverQuery {
                        sort_by: Some("popularity.desc".into()),
                        ..Default::default()
                    },
                ),
                client,
                false,
            )),
            "popular_tv" => Box::pin(with_imdb_resolved(
                discover_tv_stream(
                    client.clone(),
                    sdks::tmdb::DiscoverQuery {
                        sort_by: Some("popularity.desc".into()),
                        ..Default::default()
                    },
                ),
                client,
                true,
            )),
            "top_rated_movies" => Box::pin(with_imdb_resolved(
                discover_movie_stream(
                    client.clone(),
                    sdks::tmdb::DiscoverQuery {
                        sort_by: Some("vote_average.desc".into()),
                        vote_count_gte: Some(300),
                        ..Default::default()
                    },
                ),
                client,
                false,
            )),
            "top_rated_tv" => Box::pin(with_imdb_resolved(
                discover_tv_stream(
                    client.clone(),
                    sdks::tmdb::DiscoverQuery {
                        sort_by: Some("vote_average.desc".into()),
                        vote_count_gte: Some(300),
                        ..Default::default()
                    },
                ),
                client,
                true,
            )),
            "trending_movies_week" => Box::pin(with_imdb_resolved(
                trending_movie_stream(client.clone(), sdks::tmdb::TrendingWindow::Week),
                client,
                false,
            )),
            "trending_tv_week" => Box::pin(with_imdb_resolved(
                trending_tv_stream(client.clone(), sdks::tmdb::TrendingWindow::Week),
                client,
                true,
            )),
            _ => return Ok(None),
        };

        Ok(Some(stream))
    }
}

// ---------------------------------------------------------------------------
// MetricsAddon
// ---------------------------------------------------------------------------

#[async_trait]
impl MetricsAddon for TmdbAddon {
    async fn metric(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetricSnapshot>> {
        let Some(tmdb_id) = media
            .external_ids
            .tmdb
        else {
            return Ok(None);
        };
        let config = crate::db::Settings::get_config(&ctx.db).await?;
        let client = tmdb_client(
            config.get_tmdb_key(),
            &ctx.config
                .tmdb_base_url,
        )?;
        let today = chrono::Utc::now().date_naive();
        let (movie_max, tv_max) = self
            .popularity_max(ctx)
            .await;

        let popularity: Option<(f64, f64)> = match media.kind {
            db::MediaKind::Movie => client
                .execute(sdks::tmdb::MovieEndpoint {
                    id: tmdb_id,
                    language: None,
                    append_to_response: vec![],
                })
                .await
                .ok()
                .and_then(|m| m.popularity)
                .map(|p| (p, movie_max)),
            db::MediaKind::Series => client
                .execute(sdks::tmdb::SeriesEndpoint {
                    id: tmdb_id,
                    language: None,
                    append_to_response: vec![],
                })
                .await
                .ok()
                .map(|s| (s.popularity, tv_max)),
            _ => return Ok(None),
        };

        let external_id = format!("tmdb:{}", tmdb_id);
        Ok(popularity.map(|(p, max)| MetricSnapshot {
            source: "tmdb".to_string(),
            media_id: Some(media.id),
            media_raw: Some(external_id.clone()),
            external_id,
            value: MetricValue::from_raw(p, max),
            date: today,
        }))
    }
}

fn movie_result_to_stub(m: sdks::tmdb::MovieSearchResult) -> db::Media {
    let id =
        common::stable_media_uuid(&db::MediaKind::Movie, &format!("tmdb:{}", m.id));
    let mut media = db::Media {
        id,
        title: m.title,
        kind: db::MediaKind::Movie,
        released_at: m
            .release_date
            .and_then(|d| d.and_hms_opt(0, 0, 0)),
        external_ids: db::ExternalIds {
            tmdb: Some(m.id),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Some(url) = tmdb_image(
        m.poster_path
            .as_deref(),
        db::ImageKind::Primary,
    ) {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

fn series_result_to_stub(s: sdks::tmdb::SeriesSearchResult) -> db::Media {
    let id =
        common::stable_media_uuid(&db::MediaKind::Series, &format!("tmdb:{}", s.id));
    let mut media = db::Media {
        id,
        title: s.name,
        kind: db::MediaKind::Series,
        released_at: s
            .first_air_date
            .and_then(|d| d.and_hms_opt(0, 0, 0)),
        external_ids: db::ExternalIds {
            tmdb: Some(s.id),
            ..Default::default()
        },
        ..Default::default()
    };
    if let Some(url) = tmdb_image(
        s.poster_path
            .as_deref(),
        db::ImageKind::Primary,
    ) {
        media.set_image(db::ImageKind::Primary, url);
    }
    media
}

/// Wraps a catalog stub stream and resolves the IMDB ID for each item inline,
/// recomputing the stable UUID from the IMDB ID. Items that cannot be resolved
/// are dropped (no IMDB ID = no canonical identity).
fn with_imdb_resolved(
    stream: impl Stream<Item = db::Media> + Send + 'static,
    client: sdks::RestClient<sdks::BearerAuth>,
    is_tv: bool,
) -> impl Stream<Item = db::Media> + Send {
    stream
        .map(move |mut stub| {
            let c = client.clone();
            async move {
                let imdb = resolve_imdb_from_ids(&stub.external_ids, is_tv, &c).await?;
                stub.id = common::stable_media_uuid(&stub.kind, imdb.as_str());
                stub.external_ids
                    .imdb = Some(imdb);
                Some(stub)
            }
        })
        .buffer_unordered(10)
        .filter_map(futures::future::ready)
}

fn discover_movie_stream(
    client: sdks::RestClient<sdks::BearerAuth>,
    query: sdks::tmdb::DiscoverQuery,
) -> impl Stream<Item = db::Media> + Send {
    futures::stream::unfold(
        (client, query, Some(1u32)),
        |(client, query, maybe_page)| async move {
            let page = maybe_page?;
            let page_query = sdks::tmdb::DiscoverQuery {
                page: Some(page),
                ..query.clone()
            };
            let resp = client
                .execute(sdks::tmdb::DiscoverMovieEndpoint { query: page_query })
                .await
                .ok()?;
            if resp
                .results
                .is_empty()
            {
                return None;
            }
            let items: Vec<db::Media> = resp
                .results
                .into_iter()
                .map(movie_result_to_stub)
                .collect();
            let next = (page < resp.total_pages).then_some(page + 1);
            Some((futures::stream::iter(items), (client, query, next)))
        },
    )
    .flatten()
}

fn discover_tv_stream(
    client: sdks::RestClient<sdks::BearerAuth>,
    query: sdks::tmdb::DiscoverQuery,
) -> impl Stream<Item = db::Media> + Send {
    futures::stream::unfold(
        (client, query, Some(1u32)),
        |(client, query, maybe_page)| async move {
            let page = maybe_page?;
            let page_query = sdks::tmdb::DiscoverQuery {
                page: Some(page),
                ..query.clone()
            };
            let resp = client
                .execute(sdks::tmdb::DiscoverTvEndpoint { query: page_query })
                .await
                .ok()?;
            if resp
                .results
                .is_empty()
            {
                return None;
            }
            let items: Vec<db::Media> = resp
                .results
                .into_iter()
                .map(series_result_to_stub)
                .collect();
            let next = (page < resp.total_pages).then_some(page + 1);
            Some((futures::stream::iter(items), (client, query, next)))
        },
    )
    .flatten()
}

fn trending_movie_stream(
    client: sdks::RestClient<sdks::BearerAuth>,
    window: sdks::tmdb::TrendingWindow,
) -> impl Stream<Item = db::Media> + Send {
    futures::stream::unfold(
        (client, Some(1u32)),
        move |(client, maybe_page)| async move {
            let page = maybe_page?;
            let resp = client
                .execute(sdks::tmdb::TrendingMovieEndpoint {
                    window,
                    page: Some(page),
                })
                .await
                .ok()?;
            if resp
                .results
                .is_empty()
            {
                return None;
            }
            let items: Vec<db::Media> = resp
                .results
                .into_iter()
                .map(movie_result_to_stub)
                .collect();
            let next = (page < resp.total_pages).then_some(page + 1);
            Some((futures::stream::iter(items), (client, next)))
        },
    )
    .flatten()
}

fn trending_tv_stream(
    client: sdks::RestClient<sdks::BearerAuth>,
    window: sdks::tmdb::TrendingWindow,
) -> impl Stream<Item = db::Media> + Send {
    futures::stream::unfold(
        (client, Some(1u32)),
        move |(client, maybe_page)| async move {
            let page = maybe_page?;
            let resp = client
                .execute(sdks::tmdb::TrendingTvEndpoint {
                    window,
                    page: Some(page),
                })
                .await
                .ok()?;
            if resp
                .results
                .is_empty()
            {
                return None;
            }
            let items: Vec<db::Media> = resp
                .results
                .into_iter()
                .map(series_result_to_stub)
                .collect();
            let next = (page < resp.total_pages).then_some(page + 1);
            Some((futures::stream::iter(items), (client, next)))
        },
    )
    .flatten()
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
            db::ImageKind::Primary,
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
            db::ImageKind::Primary,
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

fn tmdb_client(
    api_key: &str,
    base_url: &str,
) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    Ok(
        sdks::RestClient::new(base_url)?.with_auth(sdks::BearerAuth {
            token: api_key.to_string(),
        }),
    )
}

async fn tmdb_client_from_ctx(
    ctx: &AppContext,
) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )
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

    let series_imdb: db::NonEmptyString = tv
        .external_ids
        .as_ref()
        .and_then(|e| {
            e.imdb_id
                .as_deref()
        })
        .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok())
        .or_else(|| {
            series
                .external_ids
                .imdb
                .clone()
        })
        .unwrap_or_else(|| {
            db::NonEmptyString::try_new(format!("tmdb:{}", tmdb_id)).unwrap()
        });

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
        .collect::<Vec<_>>();

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
        .collect::<Vec<_>>();

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
            db::ImageKind::Primary,
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
                db::ImageKind::Primary,
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

fn build_studio_relations(
    left_media_id: uuid::Uuid,
    companies: &[sdks::tmdb::ProductionCompany],
) -> Vec<(db::MediaRelation, db::Media)> {
    companies
        .iter()
        .map(|company| {
            let name = &company.name;
            let studio_id =
                common::stable_media_uuid(&db::MediaKind::Studio, &name.to_lowercase());
            (
                db::MediaRelation {
                    left_media_id,
                    right_media_id: studio_id,
                    ..Default::default()
                },
                db::Media {
                    id: studio_id,
                    title: name.clone(),
                    kind: db::MediaKind::Studio,
                    ..Default::default()
                },
            )
        })
        .collect()
}

fn build_location_relations(
    left_media_id: uuid::Uuid,
    countries: &[sdks::tmdb::ProductionCountry],
) -> Vec<(db::MediaRelation, db::Media)> {
    countries
        .iter()
        .map(|country| {
            let name = &country.name;
            let country_id = common::stable_media_uuid(
                &db::MediaKind::Country,
                &name.to_lowercase(),
            );
            (
                db::MediaRelation {
                    left_media_id,
                    right_media_id: country_id,
                    ..Default::default()
                },
                db::Media {
                    id: country_id,
                    title: name.clone(),
                    kind: db::MediaKind::Country,
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

/// Extract unique provider names from a watch/providers response for the given
/// country (falls back to "US", then to the first available country). Returns
/// tags of the form `"provider:Name"` covering flatrate, rent, and buy entries.
fn watch_provider_tags(
    resp: Option<&sdks::tmdb::WatchProvidersResponse>,
    country: &str,
) -> Vec<String> {
    let Some(resp) = resp else { return vec![] };

    let pick = resp
        .results
        .get(&country.to_uppercase())
        .or_else(|| {
            resp.results
                .get("US")
        })
        .or_else(|| {
            resp.results
                .values()
                .next()
        });

    let Some(entry) = pick else { return vec![] };

    let mut names: Vec<String> = entry
        .flatrate
        .iter()
        .chain(
            entry
                .rent
                .iter(),
        )
        .chain(
            entry
                .buy
                .iter(),
        )
        .map(|p| {
            p.provider_name
                .clone()
        })
        .collect();
    names.sort_unstable();
    names.dedup();
    names
        .into_iter()
        .map(|n| format!("provider:{}", n))
        .collect()
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

    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;

    match media.kind {
        db::MediaKind::Movie => {
            // Use the TMDB ID directly if known; otherwise discover it via /find.
            let tmdb_movie_id: Option<i64> = if let Some(id) = ids.tmdb {
                Some(id)
            } else {
                let (external_id, external_source) = if let Some(ref imdb) = ids.imdb {
                    (
                        imdb.clone()
                            .into(),
                        "imdb_id",
                    )
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
                        .as_deref()
                        .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok())
                        .or_else(|| {
                            ids.imdb
                                .clone()
                        }),
                    tvdb: ids.tvdb,
                    ..Default::default()
                };
                let logo = movie_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_logo())
                    .and_then(|p| tmdb_image(Some(p), db::ImageKind::Logo));
                let thumb = movie_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_thumb())
                    .and_then(|p| tmdb_image(Some(p), db::ImageKind::Thumb));
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
                    original_language: Some(
                        movie_details
                            .original_language
                            .clone(),
                    ),
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(
                    movie_details
                        .poster_path
                        .as_deref(),
                    db::ImageKind::Primary,
                ) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(
                    movie_details
                        .backdrop_path
                        .as_deref(),
                    db::ImageKind::Backdrop,
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
                if let Some(companies) = &movie_details.production_companies {
                    relations.extend(build_studio_relations(media.id, companies));
                }
                if let Some(countries) = &movie_details.production_countries {
                    relations.extend(build_location_relations(media.id, countries));
                }
                if !relations.is_empty() {
                    patch.relations = Some(relations);
                }
                let providers = client
                    .execute(
                        sdks::tmdb::MovieWatchProvidersEndpoint { movie_id: tmdb_id }
                            .with_cache(Duration::from_secs(86400)),
                    )
                    .await
                    .ok();
                patch.tags = watch_provider_tags(providers.as_ref(), &metadata_country);
                patch.pending_popularity = movie_details
                    .popularity
                    .map(|p| {
                        (
                            format!("tmdb:{}", tmdb_id),
                            MetricValue::from_raw(p, 1_000.0),
                        )
                    });
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Series => {
            // Use the TMDB ID directly if known; otherwise discover it via /find.
            let tmdb_series_id: Option<i64> = if let Some(id) = ids.tmdb {
                Some(id)
            } else {
                let (external_id, external_source) = if let Some(ref imdb) = ids.imdb {
                    (
                        imdb.clone()
                            .into(),
                        "imdb_id",
                    )
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
                    imdb: tmdb_ext
                        .and_then(|e| {
                            e.imdb_id
                                .as_deref()
                        })
                        .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok()),
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
                    .and_then(|p| tmdb_image(Some(p), db::ImageKind::Logo));
                let thumb = tv_details
                    .images
                    .as_ref()
                    .and_then(|i| i.best_thumb())
                    .and_then(|p| tmdb_image(Some(p), db::ImageKind::Thumb));
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
                    original_language: Some(
                        tv_details
                            .original_language
                            .clone(),
                    ),
                    ..Default::default()
                };
                if let Some(url) = tmdb_image(
                    tv_details
                        .poster_path
                        .as_deref(),
                    db::ImageKind::Primary,
                ) {
                    patch.set_image(db::ImageKind::Primary, url);
                }
                if let Some(url) = tmdb_image(
                    tv_details
                        .backdrop_path
                        .as_deref(),
                    db::ImageKind::Backdrop,
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
                if let Some(companies) = &tv_details.production_companies {
                    relations.extend(build_studio_relations(media.id, companies));
                }
                if let Some(countries) = &tv_details.production_countries {
                    relations.extend(build_location_relations(media.id, countries));
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
                            db::ImageKind::Primary,
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
                let providers = client
                    .execute(
                        sdks::tmdb::TvWatchProvidersEndpoint { series_id: tmdb_id }
                            .with_cache(Duration::from_secs(86400)),
                    )
                    .await
                    .ok();
                patch.tags = watch_provider_tags(providers.as_ref(), &metadata_country);
                patch.pending_popularity = Some((
                    format!("tmdb:{}", tmdb_id),
                    MetricValue::from_raw(tv_details.popularity, 1_000.0),
                ));
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
                                external_id: series_imdb
                                    .clone()
                                    .into(),
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
                    imdb: tmdb_ext
                        .and_then(|e| {
                            e.imdb_id
                                .as_deref()
                        })
                        .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok()),
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
                let still_url =
                    tmdb_image(best_still.as_deref(), db::ImageKind::Primary);
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
                            db::ImageKind::Primary,
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
                    imdb: details
                        .imdb_id
                        .and_then(|s| db::NonEmptyString::try_new(s).ok()),
                    ..Default::default()
                },
                ..Default::default()
            };
            if let Some(url) = tmdb_image(
                details
                    .profile_path
                    .as_deref(),
                db::ImageKind::Primary,
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
                        external_id: imdb
                            .clone()
                            .into(),
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
        db::ImageKind::Primary,
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
    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;

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
                .map(|s| format!("{}{}", "https://image.tmdb.org/t/p/original", s));
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

async fn search_tmdb_movie(
    query: &str,
    limit: usize,
    ctx: &AppContext,
) -> Result<Vec<db::Media>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;
    let resp = client
        .execute(sdks::tmdb::SearchMovieEndpoint {
            query: query.to_string(),
            year: None,
        })
        .await?;
    Ok(resp
        .results
        .into_iter()
        .take(limit)
        .map(movie_result_to_stub)
        .collect())
}

async fn search_tmdb_series(
    query: &str,
    limit: usize,
    ctx: &AppContext,
) -> Result<Vec<db::Media>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;
    let resp = client
        .execute(sdks::tmdb::SearchTvEndpoint {
            query: query.to_string(),
        })
        .await?;
    Ok(resp
        .results
        .into_iter()
        .take(limit)
        .map(series_result_to_stub)
        .collect())
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
    let client = tmdb_client(
        api_key,
        &ctx.config
            .tmdb_base_url,
    )?;

    let lookup_for_find = || -> Option<(String, &'static str)> {
        let ids = &media.external_ids;
        if let Some(tmdb_id) = ids.tmdb {
            return Some((tmdb_id.to_string(), "tmdb_id"));
        }
        if let Some(ref imdb) = ids.imdb {
            return Some((
                imdb.clone()
                    .into(),
                "imdb_id",
            ));
        }
        if let Some(ref series_imdb) = ids.series_imdb {
            return Some((
                series_imdb
                    .clone()
                    .into(),
                "imdb_id",
            ));
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
        let url = format!("https://image.tmdb.org/t/p/original{}", entry.file_path);
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
                            url: Some(format!(
                                "https://image.tmdb.org/t/p/original{p}"
                            )),
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
                            url: Some(format!(
                                "https://image.tmdb.org/t/p/original{b}"
                            )),
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
                                external_id: series_imdb
                                    .clone()
                                    .into(),
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
                        let url = format!("https://image.tmdb.org/t/p/original{p}");
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
) -> Option<db::NonEmptyString> {
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
                        .and_then(|s| db::NonEmptyString::try_new(s).ok())
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
                    if let Some(imdb) = movie
                        .imdb_id
                        .and_then(|s| db::NonEmptyString::try_new(s).ok())
                    {
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

        // FindById returns a partial object without external_ids; use the TMDB id
        // to fetch the full record which includes external_ids (via append_to_response).
        if is_tv {
            let tmdb_id = find_resp
                .tv_results
                .into_iter()
                .next()?
                .id;
            let series = client
                .execute(
                    sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                        .with_cache(Duration::from_secs(86400)),
                )
                .await
                .ok()?;
            return series
                .external_ids
                .and_then(|e| e.imdb_id)
                .and_then(|s| db::NonEmptyString::try_new(s).ok());
        } else {
            let tmdb_id = find_resp
                .movie_results
                .into_iter()
                .next()?
                .id;
            let movie = client
                .execute(
                    sdks::tmdb::MovieEndpoint::new(tmdb_id)
                        .with_cache(Duration::from_secs(86400)),
                )
                .await
                .ok()?;
            return movie
                .imdb_id
                .and_then(|s| db::NonEmptyString::try_new(s).ok());
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmdb_test_client(base_url: &str) -> sdks::RestClient<sdks::BearerAuth> {
        sdks::RestClient::new(base_url)
            .unwrap()
            .with_auth(sdks::BearerAuth {
                token: String::new(),
            })
    }

    fn mock_tv_series(server: &httpmock::MockServer, tmdb_id: i64, imdb_id: &str) {
        let imdb = imdb_id.to_string();
        server.mock(|when, then| {
            when.path(format!("/tv/{tmdb_id}"));
            then.status(200)
                .json_body(serde_json::json!({
                    "id": tmdb_id,
                    "external_ids": { "imdb_id": imdb }
                }));
        });
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_black_summoner() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/416588")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 157842, "name": "Black Summoner"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 157842, "tt21249100");

        let ids = db::ExternalIds {
            tvdb: Some(416588),
            ..Default::default()
        };
        let result =
            resolve_imdb_from_ids(&ids, true, &tmdb_test_client(&server.base_url()))
                .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt21249100"),
            "Black Summoner tvdbid-416588"
        );
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_bleach() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/74796")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 30984, "name": "Bleach"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 30984, "tt0434665");

        let ids = db::ExternalIds {
            tvdb: Some(74796),
            ..Default::default()
        };
        let result =
            resolve_imdb_from_ids(&ids, true, &tmdb_test_client(&server.base_url()))
                .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt0434665"),
            "Bleach tvdbid-74796"
        );
    }

    #[tokio::test]
    async fn resolve_imdb_from_ids_tvdb_blood_c() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/find/249864")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 43270, "name": "Blood-C"}],
                    "movie_results": []
                }));
        });
        mock_tv_series(&server, 43270, "tt1890725");

        let ids = db::ExternalIds {
            tvdb: Some(249864),
            ..Default::default()
        };
        let result =
            resolve_imdb_from_ids(&ids, true, &tmdb_test_client(&server.base_url()))
                .await;
        assert_eq!(
            result
                .as_deref()
                .map(|s| s.as_str()),
            Some("tt1890725"),
            "Blood-C tvdbid-249864"
        );
    }
}
