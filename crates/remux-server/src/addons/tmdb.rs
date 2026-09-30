use anyhow::Result;
use async_trait::async_trait;
use futures::{Stream, StreamExt};
use std::{
    collections::{BTreeSet, HashMap},
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::sync::Mutex;
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonPreset, AddonPresetRegistration,
    CatalogAddon, CatalogInfo, MediaKind, MetaAddon, ResourceType, SearchAddon,
    TreeAddon,
};
use crate::{
    AppContext, api, common, db, sdks,
    sdks::remux::MetadataLanguage,
    sdks::{CachedEndpoint, ClientError},
    services::MediaResolveService,
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
            ],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Series,
                MediaKind::Episode,
                MediaKind::Person,
            ],
            supported_resources_user: vec![ResourceType::Search],
            supported_types_user: vec![
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
        let addon = Arc::new(TmdbAddon);
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            meta: Some(addon.clone()),
            search: Some(addon.clone()),
            tree: Some(addon.clone()),
            catalog: Some(addon.clone()),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(TmdbPreset))
}

pub struct TmdbAddon;

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

    async fn rate_limit_cooldown(&self) -> Duration {
        common::tmdb_rate_limit()
            .remaining_cooldown()
            .await
    }

    async fn images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        options: super::ImageFetchOptions,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        tmdb_remote_images(ctx, media, options).await
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

    async fn search_titles_in(
        &self,
        kind: &db::MediaKind,
        query: &str,
        ctx: &AppContext,
        language: &MetadataLanguage,
    ) -> HashMap<Uuid, String> {
        search_tmdb_titles_in(kind, query, ctx, language)
            .await
            .unwrap_or_else(|error| {
                warn!(%error, %language, "TMDB translated search failed");
                HashMap::new()
            })
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
            "popular_movies" => Box::pin(discover_movie_stream(
                client,
                sdks::tmdb::DiscoverQuery {
                    sort_by: Some("popularity.desc".into()),
                    ..Default::default()
                },
            )),
            "popular_tv" => Box::pin(discover_tv_stream(
                client,
                sdks::tmdb::DiscoverQuery {
                    sort_by: Some("popularity.desc".into()),
                    ..Default::default()
                },
            )),
            "top_rated_movies" => Box::pin(discover_movie_stream(
                client,
                sdks::tmdb::DiscoverQuery {
                    sort_by: Some("vote_average.desc".into()),
                    vote_count_gte: Some(300),
                    ..Default::default()
                },
            )),
            "top_rated_tv" => Box::pin(discover_tv_stream(
                client,
                sdks::tmdb::DiscoverQuery {
                    sort_by: Some("vote_average.desc".into()),
                    vote_count_gte: Some(300),
                    ..Default::default()
                },
            )),
            "trending_movies_week" => Box::pin(trending_movie_stream(
                client,
                sdks::tmdb::TrendingWindow::Week,
            )),
            "trending_tv_week" => {
                Box::pin(trending_tv_stream(client, sdks::tmdb::TrendingWindow::Week))
            }
            _ => return Ok(None),
        };

        Ok(Some(stream))
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

fn tmdb_external_ids(
    tmdb_id: i64,
    external: Option<&sdks::tmdb::ExternalIds>,
) -> db::ExternalIds {
    db::ExternalIds {
        tmdb: Some(tmdb_id),
        imdb: external
            .and_then(|ids| {
                ids.imdb_id
                    .as_ref()
            })
            .and_then(|id| db::NonEmptyString::try_new(id.clone()).ok()),
        tvdb: external.and_then(|ids| ids.tvdb_id),
        ..Default::default()
    }
}

impl From<&sdks::tmdb::Season> for db::Media {
    fn from(s: &sdks::tmdb::Season) -> Self {
        let air_date = s
            .air_date
            .and_then(|d| d.and_hms_opt(0, 0, 0));
        let mut media = db::Media {
            kind: db::MediaKind::Season,
            title: crate::addons::season_title(s.season_number),
            description: s
                .overview
                .clone()
                .filter(|o| !o.is_empty()),
            idx: Some(s.season_number),
            external_ids: tmdb_external_ids(
                s.id,
                s.external_ids
                    .as_ref(),
            ),
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
        let external_ratings = db::ExternalRatings {
            tmdb: ep
                .vote_average
                .map(|score| db::Rating {
                    score,
                    vote_count: ep
                        .vote_count
                        .map(|v| v as u32),
                }),
            ..Default::default()
        };
        let mut media = db::Media {
            kind: db::MediaKind::Episode,
            title: ep
                .name
                .clone(),
            description: ep
                .overview
                .clone()
                .filter(|o| !o.is_empty()),
            idx: Some(ep.episode_number),
            parent_idx: Some(ep.season_number),
            external_ids: tmdb_external_ids(
                ep.id,
                ep.external_ids
                    .as_ref(),
            ),
            released_at: ep
                .air_date
                .and_then(|d| d.and_hms_opt(0, 0, 0)),
            runtime: ep
                .runtime
                .map(|r| r * 60),
            rating_audience: external_ratings.audience_rating(),
            external_ratings: Some(external_ratings),
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

    async fn rate_limit_cooldown(&self) -> Duration {
        common::tmdb_rate_limit()
            .remaining_cooldown()
            .await
    }
}

fn tmdb_client(
    api_key: &str,
    base_url: &str,
) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    Ok(sdks::RestClient::new(base_url)?
        .with_auth(sdks::BearerAuth {
            token: api_key.to_string(),
        })
        .with_retry(sdks::ExponentialBackoff::builder().build_with_max_retries(3))
        // TMDB never sends a `Retry-After` header on 429s (they dropped
        // fixed rate limiting in 2019), so without this every 429 falls
        // back to the SDK's generic 60s default, which is far longer than
        // TMDB's actual throttle window. Keep this in sync with
        // `common::tmdb_client_from_config`'s value — both share one
        // cooldown, and a 429 seen on either client installs its own
        // fallback as the block duration.
        .with_default_retry_after(std::time::Duration::from_secs(2))
        // This is the client TmdbAddon's own meta_fetch/tree/catalog use —
        // the highest-traffic TMDB path during a refresh. Without sharing
        // the same cooldown as `common::tmdb_client`, a 429 seen here would
        // never throttle calls made through that other factory, and vice
        // versa: two independent clients, two independent (and inconsistent)
        // views of whether TMDB is currently rate-limiting us.
        .with_shared_rate_limit(common::tmdb_rate_limit()))
}

async fn tmdb_client_from_ctx(
    ctx: &AppContext,
) -> Result<sdks::RestClient<sdks::BearerAuth>> {
    // Cache the client after first build to avoid querying the DB for the
    // API key on every call — critical when many concurrent refresh_meta
    // or get_tree calls all need a TMDB client.
    use std::sync::OnceLock;
    static CLIENT: OnceLock<sdks::RestClient<sdks::BearerAuth>> = OnceLock::new();
    if let Some(client) = CLIENT.get() {
        return Ok(client.clone());
    }
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;
    let _ = CLIENT.set(client.clone());
    Ok(client)
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
    let preferred_language = crate::db::Settings::get_config(&ctx.db)
        .await
        .ok()
        .and_then(|c| c.preferred_metadata_language);
    let tv = client
        .execute(
            sdks::tmdb::SeriesEndpoint::new(tmdb_id, preferred_language)
                .with_cache(Duration::from_secs(360)),
        )
        .await?;

    let series_key = series.series_canonical_key();

    let seasons: Vec<db::Media> = tv
        .seasons
        .iter()
        .map(|s| {
            let mut stub = db::Media::from(s);
            stub.id = db::Media::season_id(&series_key, s.season_number);
            stub.parent_id = Some(series.id);
            stub.grandparent_id = Some(series.id);
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
    let (Some(gp), Some(season_number)) = (
        season
            .grandparent
            .as_deref(),
        season.idx,
    ) else {
        return Ok(None);
    };
    let Some(series_tmdb_id) = gp
        .external_ids
        .tmdb
    else {
        return Ok(None);
    };
    let client = tmdb_client_from_ctx(ctx).await?;
    let preferred_language = crate::db::Settings::get_config(&ctx.db)
        .await
        .ok()
        .and_then(|c| c.preferred_metadata_language);
    let season_details = client
        .execute(
            sdks::tmdb::SeasonEndpoint {
                series_id: series_tmdb_id,
                season_number: season_number as i64,
                language: preferred_language,
                append_to_response: None,
            }
            .with_cache(Duration::from_secs(360)),
        )
        .await?;

    let anchor = gp.series_canonical_key();
    let episodes: Vec<db::Media> = season_details
        .episodes
        .unwrap_or_default()
        .into_iter()
        .map(|ep| {
            let mut stub = db::Media::from(&ep);
            stub.id =
                db::Media::episode_id(&anchor, ep.season_number, ep.episode_number);
            stub.parent_id = Some(db::Media::season_id(&anchor, ep.season_number));
            stub.grandparent_id = season.parent_id;
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

fn build_crew_relations(
    left_media_id: uuid::Uuid,
    credits: &sdks::tmdb::Credits,
) -> Vec<(db::MediaRelation, db::Media)> {
    let mut relations = Vec::new();
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
            let person_id = common::stable_media_uuid(
                &db::MediaKind::Person,
                &member
                    .id
                    .to_string(),
            );
            let mut person = db::Media {
                id: person_id,
                title: member
                    .name
                    .clone(),
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
                weight: Some(member.order as i64),
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

/// Image languages to request alongside the configured metadata language so
/// `best_logo`/`best_thumb` (which look for English-tagged title-card art)
/// can find a match even when the server's preferred language isn't English.
/// Without `include_image_language`, TMDB restricts `images.backdrops`/
/// `logos` to the request's `language` plus untagged entries, so an "en"
/// entry never comes back unless the configured language already is "en".
fn thumb_and_logo_languages(preferred_language: Option<&str>) -> String {
    let mut langs = vec!["en", "null"];
    if let Some(primary) = preferred_language.and_then(|l| {
        l.split('-')
            .next()
    }) && !primary.is_empty()
        && !langs.contains(&primary)
    {
        langs.insert(0, primary);
    }
    langs.join(",")
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
            // `resolve_external_ids` (called at the top of
            // `refresh_meta`, before any addon runs) already resolves a
            // tmdb id from whatever else is known, if one is resolvable at
            // all — nothing left to discover here.
            let tmdb_movie_id: Option<i64> = ids.tmdb;

            if let Some(tmdb_id) = tmdb_movie_id {
                let langs = translation_languages(ctx, config).await;
                let mut endpoint = sdks::tmdb::MovieEndpoint::new(
                    tmdb_id,
                    config
                        .preferred_metadata_language
                        .clone(),
                )
                .with_image_languages(
                    with_translation_image_languages(
                        thumb_and_logo_languages(
                            config
                                .preferred_metadata_language
                                .as_deref(),
                        ),
                        &langs,
                    ),
                );
                if !langs.is_empty() {
                    endpoint = endpoint.with_translations();
                }
                let movie_details = client
                    .execute(endpoint.with_cache(Duration::from_secs(360)))
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
                    ..Default::default()
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
                patch.translations = translated_texts(
                    movie_details
                        .translations
                        .as_ref(),
                    movie_details
                        .images
                        .as_ref(),
                    &langs,
                    &movie_details.original_language,
                    movie_details
                        .original_title
                        .as_deref(),
                );
                let mut relations = vec![];
                if let Some(genres) = &movie_details.genres {
                    relations.extend(build_genre_relations(media.id, genres));
                    attach_genre_translations(
                        &mut relations,
                        genres,
                        sdks::tmdb::GenreListKind::Movie,
                        &langs,
                        &client,
                    )
                    .await;
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
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Series => {
            // `resolve_external_ids` (called at the top of
            // `refresh_meta`, before any addon runs) already resolves a
            // tmdb id from whatever else is known, if one is resolvable at
            // all — nothing left to discover here.
            let tmdb_series_id: Option<i64> = ids.tmdb;

            if let Some(tmdb_id) = tmdb_series_id {
                let langs = translation_languages(ctx, config).await;
                let mut endpoint = sdks::tmdb::SeriesEndpoint::new(
                    tmdb_id,
                    config
                        .preferred_metadata_language
                        .clone(),
                )
                .with_image_languages(
                    with_translation_image_languages(
                        thumb_and_logo_languages(
                            config
                                .preferred_metadata_language
                                .as_deref(),
                        ),
                        &langs,
                    ),
                );
                if !langs.is_empty() {
                    endpoint = endpoint.with_translations();
                }
                let tv_details = client
                    .execute(endpoint.with_cache(Duration::from_secs(360)))
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
                    ..Default::default()
                };
                let tmdb_status = tv_details
                    .status
                    .as_ref();
                let status = match tmdb_status {
                    Some(sdks::tmdb::Status::Ended | sdks::tmdb::Status::Canceled) => {
                        Some(db::MediaStatus::Ended)
                    }
                    Some(
                        sdks::tmdb::Status::ReturningSeries | sdks::tmdb::Status::Pilot,
                    ) => Some(db::MediaStatus::Continuing),
                    Some(
                        sdks::tmdb::Status::InProduction
                        | sdks::tmdb::Status::Planned
                        | sdks::tmdb::Status::PostProduction,
                    ) => Some(db::MediaStatus::Unreleased),
                    _ => None,
                };
                let end_date = if matches!(
                    tmdb_status,
                    Some(sdks::tmdb::Status::Ended | sdks::tmdb::Status::Canceled)
                ) {
                    tv_details
                        .last_air_date
                        .as_deref()
                        .and_then(|s| {
                            chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
                        })
                        .and_then(|d| d.and_hms_opt(0, 0, 0))
                } else {
                    None
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
                    status,
                    end_date,
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
                patch.translations = translated_texts(
                    tv_details
                        .translations
                        .as_ref(),
                    tv_details
                        .images
                        .as_ref(),
                    &langs,
                    &tv_details.original_language,
                    Some(&tv_details.original_name),
                );
                let mut relations = vec![];
                if let Some(genres) = &tv_details.genres {
                    relations.extend(build_genre_relations(media.id, genres));
                    attach_genre_translations(
                        &mut relations,
                        genres,
                        sdks::tmdb::GenreListKind::Tv,
                        &langs,
                        &client,
                    )
                    .await;
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
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Episode => {
            let series_tmdb_id =
                MediaResolveService::stored_series_tmdb_id(media, ctx).await?;
            let season_number = media.parent_idx;
            let episode_number = media.idx;
            if let (Some(tmdb_id), Some(s_n), Some(e_n)) =
                (series_tmdb_id, season_number, episode_number)
            {
                // Use the SeasonEndpoint (already cached from get_tree) instead of
                // a separate per-episode EpisodeEndpoint call.
                // Borrow the cached season rather than taking it by value: this runs
                // once per episode, and copying every episode's overview/credits/
                // guest stars just to read one of them is quadratic per season.
                let season = client
                    .execute_arc(
                        sdks::tmdb::SeasonEndpoint {
                            series_id: tmdb_id,
                            season_number: s_n,
                            language: config
                                .preferred_metadata_language
                                .clone(),
                            append_to_response: None,
                        }
                        .with_cache(Duration::from_secs(360)),
                    )
                    .await?;
                let Some(ep) = season
                    .episodes
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .find(|e| e.episode_number == e_n)
                else {
                    return Ok(None);
                };
                let mut patch = db::Media::from(ep);
                for lang in translation_languages(ctx, config)
                    .await
                    .iter()
                {
                    let Some(season) =
                        season_in_language(&client, tmdb_id, s_n, lang).await
                    else {
                        continue;
                    };
                    if let Some(ep) = season
                        .episodes
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .find(|e| e.episode_number == e_n)
                    {
                        patch
                            .translations
                            .push(db::TranslatedText {
                                language: lang.clone(),
                                title: Some(
                                    ep.name
                                        .clone(),
                                ),
                                description: ep
                                    .overview
                                    .clone(),
                                primary_image: None,
                            });
                    }
                }
                return Ok(Some(patch));
            }
        }
        db::MediaKind::Season => {
            return fetch_tmdb_season_meta(media, ctx, &client, config).await;
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
    config: &crate::api::ServerConfiguration,
) -> Result<Option<db::Media>> {
    let series_tmdb_id = MediaResolveService::stored_series_tmdb_id(media, ctx).await?;

    let (Some(tmdb_id), Some(season_idx)) = (series_tmdb_id, media.idx) else {
        return Ok(None);
    };
    let langs = translation_languages(ctx, config).await;
    // Independent requests run concurrently, so one season costs one round trip.
    let (tv_details, season_ids, in_languages) = futures::join!(
        client.execute(
            sdks::tmdb::SeriesEndpoint::new(
                tmdb_id,
                config
                    .preferred_metadata_language
                    .clone(),
            )
            .with_cache(Duration::from_secs(360)),
        ),
        client.execute(
            sdks::tmdb::SeasonExternalIdsEndpoint {
                series_id: tmdb_id,
                season_number: season_idx,
            }
            .with_cache(Duration::from_secs(360)),
        ),
        futures::future::join_all(
            langs
                .iter()
                .map(|lang| async move {
                    (
                        lang,
                        season_in_language(client, tmdb_id, season_idx, lang).await,
                    )
                })
        ),
    );
    let tv_details = tv_details?;
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
    patch.external_ids = tmdb_external_ids(
        season.id,
        season
            .external_ids
            .as_ref(),
    );
    match season_ids {
        Ok(ids) => patch
            .external_ids
            .merge(&tmdb_external_ids(season.id, Some(&ids)), true),
        Err(error) => {
            warn!(%error, series_id = tmdb_id, season = season_idx, "TMDB season external IDs unavailable")
        }
    }
    let server_poster = season
        .poster_path
        .clone();
    for (lang, season) in in_languages {
        if let Some(season) = season {
            let primary_image = season
                .poster_path
                .as_deref()
                .filter(|p| Some(*p) != server_poster.as_deref())
                .and_then(|p| tmdb_image(Some(p), db::ImageKind::Primary));
            patch
                .translations
                .push(db::TranslatedText {
                    language: lang.clone(),
                    title: Some(
                        season
                            .name
                            .clone(),
                    ),
                    description: season
                        .overview
                        .clone(),
                    primary_image,
                });
        }
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
            language: config
                .preferred_metadata_language
                .clone(),
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
            language: config
                .preferred_metadata_language
                .clone(),
        })
        .await?;
    Ok(resp
        .results
        .into_iter()
        .take(limit)
        .map(series_result_to_stub)
        .collect())
}

/// Titles of a movie/series search's results in `language`, keyed by stub
/// id. Where TMDB has no translation it returns the original title; those
/// are left out so the server-language title is shown instead, unless
/// `language` is the original language.
async fn search_tmdb_titles_in(
    kind: &db::MediaKind,
    query: &str,
    ctx: &AppContext,
    language: &MetadataLanguage,
) -> Result<HashMap<Uuid, String>> {
    let config = crate::db::Settings::get_config(&ctx.db).await?;
    let client = tmdb_client(
        config.get_tmdb_key(),
        &ctx.config
            .tmdb_base_url,
    )?;
    let param = Some(sdks::tmdb::language_param(language));
    let results: Vec<(i64, String, Option<String>, Option<String>)> = match kind {
        db::MediaKind::Movie => client
            .execute(sdks::tmdb::SearchMovieEndpoint {
                query: query.to_string(),
                year: None,
                language: param,
            })
            .await?
            .results
            .into_iter()
            .map(|m| (m.id, m.title, m.original_title, m.original_language))
            .collect(),
        db::MediaKind::Series => client
            .execute(sdks::tmdb::SearchTvEndpoint {
                query: query.to_string(),
                language: param,
            })
            .await?
            .results
            .into_iter()
            .map(|s| (s.id, s.name, s.original_name, s.original_language))
            .collect(),
        _ => return Ok(HashMap::new()),
    };
    Ok(results
        .into_iter()
        .filter(|(_, title, original, original_language)| {
            !title
                .trim()
                .is_empty()
                && (original.as_deref() != Some(title.as_str())
                    || original_language
                        .as_deref()
                        .is_some_and(|l| l.eq_ignore_ascii_case(language.base())))
        })
        .map(|(id, title, _, _)| {
            (
                common::stable_media_uuid(kind, &format!("tmdb:{id}")),
                title,
            )
        })
        .collect())
}

// ---------------------------------------------------------------------------
// Translations for users reading in other languages
// ---------------------------------------------------------------------------

/// Store key for the cached active-language set; cleared when a user's
/// metadata language or the server configuration changes.
pub(crate) const TRANSLATION_LANGUAGES_CACHE_KEY: &str = "tmdb:translation_languages";

/// Languages to fetch translations in. Cached briefly so a refresh batch
/// reads the users table once rather than once per item.
async fn translation_languages(
    ctx: &AppContext,
    config: &crate::api::ServerConfiguration,
) -> Arc<BTreeSet<MetadataLanguage>> {
    const KEY: &str = TRANSLATION_LANGUAGES_CACHE_KEY;
    if let Some(langs) = ctx
        .store
        .get::<BTreeSet<MetadataLanguage>>(KEY)
    {
        return langs;
    }
    let langs = db::active_metadata_languages(
        &ctx.db,
        config
            .preferred_metadata_language
            .as_deref(),
    )
    .await
    .unwrap_or_else(|e| {
        warn!(error = %e, "failed to load active metadata languages");
        BTreeSet::new()
    });
    ctx.store
        .save(KEY, langs.clone(), Duration::from_secs(60));
    Arc::new(langs)
}

/// `include_image_language` for a detail call: `base` plus every active
/// language, so posters in those languages come back.
fn with_translation_image_languages(
    base: String,
    langs: &BTreeSet<MetadataLanguage>,
) -> String {
    let mut out: Vec<&str> = base
        .split(',')
        .collect();
    for lang in langs {
        if !out.contains(&lang.base()) {
            out.insert(out.len() - 1, lang.base());
        }
    }
    out.join(",")
}

/// Best-voted poster tagged with `language`'s base language.
fn poster_in_language(
    images: Option<&sdks::tmdb::Images>,
    language: &MetadataLanguage,
) -> Option<String> {
    let poster = images?
        .posters
        .iter()
        .filter(|p| {
            p.iso_639_1
                .as_deref()
                .is_some_and(|l| l.eq_ignore_ascii_case(language.base()))
        })
        .max_by(|a, b| {
            a.vote_average
                .unwrap_or_default()
                .total_cmp(
                    &b.vote_average
                        .unwrap_or_default(),
                )
                .then(
                    a.vote_count
                        .cmp(&b.vote_count),
                )
        })?;
    tmdb_image(Some(&poster.file_path), db::ImageKind::Primary)
}

/// TMDB leaves a translation's title empty when it equals the original
/// title, so a reader of the original language gets `original_title`.
fn translated_texts(
    translations: Option<&sdks::tmdb::Translations>,
    images: Option<&sdks::tmdb::Images>,
    langs: &BTreeSet<MetadataLanguage>,
    original_language: &str,
    original_title: Option<&str>,
) -> Vec<db::TranslatedText> {
    langs
        .iter()
        .filter_map(|lang| {
            let (title, description) = translations
                .map(|t| t.text_for(lang))
                .unwrap_or_default();
            let title = title.or_else(|| {
                lang.base()
                    .eq_ignore_ascii_case(original_language)
                    .then(|| original_title.map(str::to_string))
                    .flatten()
            });
            let primary_image = poster_in_language(images, lang);
            (title.is_some() || description.is_some() || primary_image.is_some()).then(
                || db::TranslatedText {
                    language: lang.clone(),
                    title,
                    description,
                    primary_image,
                },
            )
        })
        .collect()
}

/// Give each genre relation row its name in every active language, matched
/// by TMDB genre id.
async fn attach_genre_translations(
    relations: &mut [(db::MediaRelation, db::Media)],
    genres: &[sdks::tmdb::Genre],
    kind: sdks::tmdb::GenreListKind,
    langs: &BTreeSet<MetadataLanguage>,
    client: &sdks::RestClient<sdks::BearerAuth>,
) {
    if langs.is_empty() || genres.is_empty() {
        return;
    }
    let genre_row_ids: Vec<(u64, Uuid)> = genres
        .iter()
        .map(|g| {
            (
                g.id,
                common::stable_media_uuid(
                    &db::MediaKind::Genre,
                    &g.name
                        .to_lowercase(),
                ),
            )
        })
        .collect();
    for lang in langs {
        let list = match client
            .execute_arc(
                sdks::tmdb::GenreListEndpoint {
                    kind,
                    language: sdks::tmdb::language_param(lang),
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
        {
            Ok(list) => list,
            Err(error) => {
                warn!(%error, %lang, "TMDB genre list unavailable");
                continue;
            }
        };
        for (tmdb_genre_id, row_id) in &genre_row_ids {
            let Some(name) = list
                .genres
                .iter()
                .find(|g| g.id == *tmdb_genre_id)
                .map(|g| {
                    g.name
                        .clone()
                })
            else {
                continue;
            };
            for (_, genre) in relations
                .iter_mut()
                .filter(|(_, m)| m.id == *row_id)
            {
                genre
                    .translations
                    .push(db::TranslatedText {
                        language: lang.clone(),
                        title: Some(name.clone()),
                        description: None,
                        primary_image: None,
                    });
            }
        }
    }
}

/// A season in one active language: season and episode names/overviews.
/// Cached like the default-language season, so every episode of a season
/// shares one call per language.
async fn season_in_language(
    client: &sdks::RestClient<sdks::BearerAuth>,
    series_id: i64,
    season_number: i64,
    lang: &MetadataLanguage,
) -> Option<Arc<sdks::tmdb::Season>> {
    client
        .execute_arc(
            sdks::tmdb::SeasonEndpoint {
                series_id,
                season_number,
                language: Some(sdks::tmdb::language_param(lang)),
                append_to_response: None,
            }
            .with_cache(Duration::from_secs(360)),
        )
        .await
        .inspect_err(|error| {
            warn!(%error, series_id, season_number, %lang, "TMDB translated season unavailable")
        })
        .ok()
}

// ---------------------------------------------------------------------------
// Remote images
// ---------------------------------------------------------------------------

fn tmdb_remote_image_languages(
    image_type: Option<&api::ImageType>,
    include_all_languages: bool,
) -> Option<&'static str> {
    (!include_all_languages && matches!(image_type, Some(api::ImageType::Backdrop)))
        .then_some("null")
}

fn tmdb_remote_metadata_language(
    preferred_language: Option<&str>,
    include_all_languages: bool,
) -> Option<String> {
    (!include_all_languages)
        .then(|| preferred_language.map(str::to_string))
        .flatten()
}

fn map_remote_image(
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

fn extend_from_tmdb_images(
    out: &mut Vec<api::RemoteImageInfo>,
    images: &sdks::tmdb::Images,
    language_neutral_backdrops: bool,
) {
    out.extend(
        images
            .backdrops
            .iter()
            .filter(|entry| {
                !language_neutral_backdrops
                    || entry
                        .iso_639_1
                        .is_none()
            })
            .map(|entry| map_remote_image("Backdrop", entry)),
    );
    out.extend(
        images
            .posters
            .iter()
            .map(|entry| map_remote_image("Primary", entry)),
    );
    out.extend(
        images
            .logos
            .iter()
            .map(|entry| map_remote_image("Logo", entry)),
    );
    out.extend(
        images
            .stills
            .iter()
            .filter(|entry| {
                !language_neutral_backdrops
                    || entry
                        .iso_639_1
                        .is_none()
            })
            .map(|entry| map_remote_image("Backdrop", entry)),
    );
    out.extend(
        images
            .stills
            .iter()
            .map(|entry| map_remote_image("Screenshot", entry)),
    );
    out.extend(
        images
            .stills
            .iter()
            .map(|entry| map_remote_image("Thumb", entry)),
    );
}

async fn tmdb_remote_images(
    ctx: &AppContext,
    media: &db::Media,
    options: super::ImageFetchOptions,
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
    let image_languages = tmdb_remote_image_languages(
        options
            .image_type
            .as_ref(),
        options.include_all_languages,
    );
    let language_neutral_backdrops = image_languages.is_some();
    let preferred_language = tmdb_remote_metadata_language(
        config
            .preferred_metadata_language
            .as_deref(),
        options.include_all_languages,
    );

    let mut out = Vec::new();

    match media.kind {
        db::MediaKind::Movie => {
            let tmdb_id = if let Some(id) = media
                .external_ids
                .tmdb
            {
                Some(id)
            } else if let Some((external_id, external_source)) =
                MediaResolveService::tmdb_search_key(&media.external_ids, None).await
            {
                MediaResolveService::find_tmdb_id_by(
                    external_id,
                    external_source,
                    false,
                    &client,
                )
                .await?
            } else {
                None
            };
            if let Some(tmdb_id) = tmdb_id {
                let mut endpoint =
                    sdks::tmdb::MovieEndpoint::new(tmdb_id, preferred_language.clone());
                if let Some(languages) = image_languages {
                    endpoint = endpoint.with_image_languages(languages);
                }
                let movie = client
                    .execute(endpoint.with_cache(Duration::from_secs(360)))
                    .await?;
                if let Some(images) = &movie.images {
                    extend_from_tmdb_images(
                        &mut out,
                        images,
                        language_neutral_backdrops,
                    );
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
            } else if let Some((external_id, external_source)) =
                MediaResolveService::tmdb_search_key(&media.external_ids, None).await
            {
                MediaResolveService::find_tmdb_id_by(
                    external_id,
                    external_source,
                    true,
                    &client,
                )
                .await?
            } else {
                None
            };
            if let Some(tmdb_id) = tmdb_id {
                let mut endpoint = sdks::tmdb::SeriesEndpoint::new(
                    tmdb_id,
                    preferred_language.clone(),
                );
                if let Some(languages) = image_languages {
                    endpoint = endpoint.with_image_languages(languages);
                }
                let tv = client
                    .execute(endpoint.with_cache(Duration::from_secs(360)))
                    .await?;
                if let Some(images) = &tv.images {
                    extend_from_tmdb_images(
                        &mut out,
                        images,
                        language_neutral_backdrops,
                    );
                }
            }
        }
        db::MediaKind::Episode => {
            let series_tmdb_id =
                MediaResolveService::stored_series_tmdb_id(media, ctx).await?;
            if let (Some(tmdb_id), Some(s_n), Some(e_n)) =
                (series_tmdb_id, media.parent_idx, media.idx)
            {
                let mut endpoint = sdks::tmdb::EpisodeEndpoint::new(
                    tmdb_id,
                    s_n,
                    e_n,
                    preferred_language,
                );
                if let Some(languages) = image_languages {
                    endpoint = endpoint.with_image_languages(languages);
                }
                let ep = client
                    .execute(endpoint.with_cache(Duration::from_secs(360)))
                    .await?;
                if let Some(images) = ep
                    .as_ref()
                    .and_then(|e| {
                        e.images
                            .as_ref()
                    })
                {
                    extend_from_tmdb_images(
                        &mut out,
                        images,
                        language_neutral_backdrops,
                    );
                }
                if out
                    .iter()
                    .all(|i| {
                        i.type_
                            .as_deref()
                            != Some("Thumb")
                    })
                {
                    if let Some(p) = ep
                        .as_ref()
                        .and_then(|e| {
                            e.still_path
                                .as_ref()
                        })
                    {
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

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::Endpoint;

    #[test]
    fn season_and_episode_external_ids_are_mapped() {
        let ids = sdks::tmdb::ExternalIds {
            imdb_id: Some("tt1234567".into()),
            tvdb_id: Some(456),
        };
        let season = sdks::tmdb::Season {
            id: 123,
            external_ids: Some(ids.clone()),
            ..Default::default()
        };
        let episode = sdks::tmdb::Episode {
            id: 789,
            external_ids: Some(ids),
            ..Default::default()
        };
        for (patch, tmdb) in [
            (db::Media::from(&season), 123),
            (db::Media::from(&episode), 789),
        ] {
            assert_eq!(
                patch
                    .external_ids
                    .tmdb,
                Some(tmdb)
            );
            assert_eq!(
                patch
                    .external_ids
                    .tvdb,
                Some(456)
            );
            assert_eq!(
                patch
                    .external_ids
                    .imdb
                    .as_deref()
                    .map(String::as_str),
                Some("tt1234567")
            );
        }
    }

    #[test]
    fn external_id_refresh_preserves_missing_values_and_respects_force() {
        let existing = db::ExternalIds {
            tmdb: Some(123),
            tvdb: Some(456),
            ..Default::default()
        };
        for force in [false, true] {
            let mut media = db::Media {
                external_ids: existing.clone(),
                ..Default::default()
            };
            let empty = sdks::tmdb::ExternalIds {
                imdb_id: Some(String::new()),
                tvdb_id: None,
            };
            super::super::apply_meta(
                &mut media,
                db::Media {
                    external_ids: tmdb_external_ids(123, Some(&empty)),
                    ..Default::default()
                },
                force,
            );
            assert_eq!(
                media
                    .external_ids
                    .tvdb,
                Some(456)
            );
            assert!(
                media
                    .external_ids
                    .imdb
                    .is_none()
            );
            super::super::apply_meta(
                &mut media,
                db::Media {
                    external_ids: tmdb_external_ids(
                        123,
                        Some(&sdks::tmdb::ExternalIds {
                            imdb_id: Some("tt1234567".into()),
                            tvdb_id: Some(999),
                        }),
                    ),
                    ..Default::default()
                },
                force,
            );
            assert_eq!(
                media
                    .external_ids
                    .tvdb,
                Some(if force { 999 } else { 456 })
            );
            assert_eq!(
                media
                    .external_ids
                    .imdb
                    .as_deref()
                    .map(String::as_str),
                Some("tt1234567")
            );
        }
    }

    fn image_entry(path: &str, language: Option<&str>) -> sdks::tmdb::ImageEntry {
        sdks::tmdb::ImageEntry {
            file_path: path.to_string(),
            iso_639_1: language.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn remote_backdrops_only_include_language_neutral_images() {
        let images = sdks::tmdb::Images {
            backdrops: vec![
                image_entry("/neutral.jpg", None),
                image_entry("/localized.jpg", Some("en")),
            ],
            posters: vec![
                image_entry("/localized-poster.jpg", Some("en")),
                image_entry("/neutral-poster.jpg", None),
            ],
            logos: vec![
                image_entry("/localized-logo.svg", Some("en")),
                image_entry("/neutral-logo.svg", None),
            ],
            ..Default::default()
        };
        let mut remote_images = Vec::new();

        extend_from_tmdb_images(&mut remote_images, &images, true);

        let backdrop_urls = remote_images
            .iter()
            .filter(|image| {
                image
                    .type_
                    .as_deref()
                    == Some("Backdrop")
            })
            .filter_map(|image| {
                image
                    .url
                    .as_deref()
            })
            .collect::<Vec<_>>();
        assert_eq!(
            backdrop_urls,
            ["https://image.tmdb.org/t/p/original/neutral.jpg"]
        );
        let primary_count = remote_images
            .iter()
            .filter(|image| {
                image
                    .type_
                    .as_deref()
                    == Some("Primary")
            })
            .count();
        assert_eq!(primary_count, 2);

        let mut all_languages = Vec::new();
        extend_from_tmdb_images(&mut all_languages, &images, false);
        assert_eq!(
            all_languages
                .iter()
                .filter(|image| image
                    .type_
                    .as_deref()
                    == Some("Backdrop"))
                .count(),
            2
        );
    }

    #[test]
    fn remote_image_queries_respect_include_all_languages() {
        assert_eq!(
            tmdb_remote_image_languages(Some(&api::ImageType::Backdrop), false),
            Some("null")
        );
        assert_eq!(
            tmdb_remote_image_languages(Some(&api::ImageType::Primary), false),
            None
        );
        assert_eq!(
            tmdb_remote_image_languages(Some(&api::ImageType::Backdrop), true),
            None
        );
        assert_eq!(tmdb_remote_image_languages(None, false), None);
        assert_eq!(
            tmdb_remote_metadata_language(Some("nl-NL"), false),
            Some("nl-NL".to_string())
        );
        assert_eq!(tmdb_remote_metadata_language(Some("nl-NL"), true), None);

        let backdrop_query =
            sdks::tmdb::MovieEndpoint::new(1, Some("nl-NL".to_string()))
                .with_image_languages("null")
                .query();
        let primary_query =
            sdks::tmdb::MovieEndpoint::new(1, Some("nl-NL".to_string())).query();

        assert!(
            backdrop_query.contains(&("include_image_language".into(), "null".into()))
        );
        assert!(
            primary_query
                .iter()
                .all(|(key, _)| key != "include_image_language")
        );
    }

    /// TMDB restricts `images.backdrops`/`logos` to the request's `language`
    /// plus untagged entries, so an "en"-tagged title card never comes back
    /// unless "en" is explicitly requested — regardless of the server's
    /// configured metadata language.
    #[test]
    fn thumb_and_logo_languages_always_include_english_and_null() {
        assert_eq!(thumb_and_logo_languages(None), "en,null");
        assert_eq!(thumb_and_logo_languages(Some("en")), "en,null");
        assert_eq!(thumb_and_logo_languages(Some("nl")), "nl,en,null");
        assert_eq!(thumb_and_logo_languages(Some("nl-NL")), "nl,en,null");
    }

    async fn ctx_with_tmdb(
        tmdb: &httpmock::MockServer,
    ) -> crate::integration_test::TestGuard {
        crate::integration_test::new_test_server_with_config(crate::Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            disable_dht: true,
            tmdb_base_url: tmdb.base_url(),
            ..Default::default()
        })
        .await
        .unwrap()
        .1
    }

    async fn set_user_language(ctx: &AppContext, language: &str) {
        sqlx::query(
            "UPDATE users SET configuration = json_set(COALESCE(configuration, '{}'), \
             '$.remux', json_object('metadata_language', ?))",
        )
        .bind(language)
        .execute(&ctx.db)
        .await
        .unwrap();
        ctx.store
            .delete(TRANSLATION_LANGUAGES_CACHE_KEY);
    }

    async fn stored_translations(
        ctx: &AppContext,
        media_id: Uuid,
    ) -> Vec<(String, Option<String>, Option<String>)> {
        sqlx::query_as(
            "SELECT language, title, description FROM media_translations \
             WHERE media_id = ? ORDER BY language",
        )
        .bind(media_id)
        .fetch_all(&ctx.db)
        .await
        .unwrap()
    }

    fn mock_spirited_away(tmdb: &httpmock::MockServer) {
        mock_spirited_away_after(tmdb, Duration::ZERO);
    }

    fn mock_spirited_away_after(
        tmdb: &httpmock::MockServer,
        delay: Duration,
    ) -> httpmock::Mock<'_> {
        let movie = tmdb.mock(|when, then| {
            when.path("/movie/129");
            then.status(200)
                .delay(delay)
                .json_body(serde_json::json!({
                    "id": 129,
                    "title": "Spirited Away",
                    "overview": "A girl enters the spirit world.",
                    "adult": false,
                    "original_language": "ja",
                    "genres": [{ "id": 16, "name": "Animation" }],
                    "translations": { "translations": [
                        { "iso_639_1": "es", "iso_3166_1": "ES",
                          "data": { "title": "El viaje de Chihiro", "overview": "Chihiro entra al mundo de los espíritus." } },
                        { "iso_639_1": "fr", "iso_3166_1": "FR",
                          "data": { "title": "Le Voyage de Chihiro", "overview": "Chihiro entre dans le monde des esprits." } }
                    ]}
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/genre/movie/list")
                .query_param("language", "es");
            then.status(200)
                .json_body(serde_json::json!({
                    "genres": [{ "id": 16, "name": "Animación" }]
                }));
        });
        movie
    }

    async fn refresh_spirited_away(ctx: &AppContext) -> Uuid {
        let movie = db::Media {
            id: common::stable_media_uuid(&db::MediaKind::Movie, "tmdb:129"),
            title: "Spirited Away".into(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                tmdb: Some(129),
                ..Default::default()
            },
            ..Default::default()
        };
        db::Media::upsert(&ctx.db, &[movie.clone()])
            .await
            .unwrap();
        ctx.addons
            .process_meta_batch(vec![movie.clone()], ctx, true, None)
            .await
            .unwrap();
        movie.id
    }

    #[tokio::test]
    async fn refresh_stores_translations_only_in_languages_users_read() {
        let tmdb = httpmock::MockServer::start();
        mock_spirited_away(&tmdb);
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;
        set_user_language(ctx, "es").await;

        let movie_id = refresh_spirited_away(ctx).await;

        assert_eq!(
            stored_translations(ctx, movie_id).await,
            vec![(
                "es".to_string(),
                Some("El viaje de Chihiro".to_string()),
                Some("Chihiro entra al mundo de los espíritus.".to_string())
            )],
            "only the language a user reads is kept"
        );
        let genre_id = common::stable_media_uuid(&db::MediaKind::Genre, "animation");
        assert_eq!(
            stored_translations(ctx, genre_id).await,
            vec![("es".to_string(), Some("Animación".to_string()), None)],
            "genre row keeps its identity and gains a translated name"
        );
    }

    /// Languages stored for Spirited Away after a full refresh during which
    /// the user's language changes from `before` to `during`. A `before`
    /// translation is stored first, as an earlier refresh would have.
    async fn full_refresh_changing_language(before: &str, during: &str) -> Vec<String> {
        let tmdb = httpmock::MockServer::start();
        let movie_mock = mock_spirited_away_after(&tmdb, Duration::from_millis(500));
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = guard
            .0
            .clone();
        set_user_language(&ctx, before).await;
        let movie_id = common::stable_media_uuid(&db::MediaKind::Movie, "tmdb:129");
        db::Media::upsert(
            &ctx.db,
            &[db::Media {
                id: movie_id,
                title: "Spirited Away".into(),
                kind: db::MediaKind::Movie,
                external_ids: db::ExternalIds {
                    tmdb: Some(129),
                    ..Default::default()
                },
                ..Default::default()
            }],
        )
        .await
        .unwrap();
        if let Ok(language) = before.parse() {
            db::MediaTranslation::upsert_provider(
                &ctx.db,
                &[(
                    movie_id,
                    db::TranslatedText {
                        language,
                        title: Some("Spirited Away (earlier)".into()),
                        description: None,
                        primary_image: None,
                    },
                )],
            )
            .await
            .unwrap();
        }
        let tasks = Arc::new(
            crate::tasks::TaskService::new(ctx.clone())
                .await
                .unwrap(),
        );
        let run = tokio::spawn({
            let ctx = ctx.clone();
            async move {
                use crate::tasks::Task;
                crate::tasks::RefreshAllMetaTask
                    .run(
                        ctx,
                        tasks,
                        crate::common::ProgressReporter::new(Default::default()),
                    )
                    .await
            }
        });
        while movie_mock.hits() == 0 {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        set_user_language(&ctx, during).await;
        run.await
            .unwrap()
            .unwrap();
        stored_translations(&ctx, movie_id)
            .await
            .into_iter()
            .map(|(language, ..)| language)
            .collect()
    }

    #[tokio::test]
    async fn full_refresh_goes_round_again_for_a_language_selected_mid_run() {
        assert_eq!(full_refresh_changing_language("", "es").await, vec!["es"]);
    }

    #[tokio::test]
    async fn full_refresh_prunes_a_language_deselected_mid_run() {
        assert!(
            full_refresh_changing_language("fr", "")
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn refresh_without_user_languages_stores_nothing() {
        let tmdb = httpmock::MockServer::start();
        mock_spirited_away(&tmdb);
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;

        let movie_id = refresh_spirited_away(ctx).await;

        assert!(
            stored_translations(ctx, movie_id)
                .await
                .is_empty()
        );
    }

    #[test]
    fn original_language_readers_get_the_original_title() {
        let translations: sdks::tmdb::Translations = serde_json::from_value(serde_json::json!({
            "translations": [
                { "iso_639_1": "en", "iso_3166_1": "US",
                  "data": { "title": "", "overview": "Spanning the years 1945 to 1955…" } },
                { "iso_639_1": "fr", "iso_3166_1": "FR",
                  "data": { "title": "", "overview": "En 1945, à New York…" } }
            ]
        }))
        .unwrap();
        let langs: BTreeSet<MetadataLanguage> = ["en", "fr"]
            .iter()
            .map(|l| {
                l.parse()
                    .unwrap()
            })
            .collect();
        let texts = translated_texts(
            Some(&translations),
            None,
            &langs,
            "en",
            Some("The Godfather"),
        );
        let title = |tag: &str| {
            texts
                .iter()
                .find(|t| {
                    t.language
                        .as_str()
                        == tag
                })
                .and_then(|t| {
                    t.title
                        .clone()
                })
        };
        assert_eq!(title("en").as_deref(), Some("The Godfather"));
        assert_eq!(
            title("fr"),
            None,
            "other languages fall back to the server title"
        );
    }

    #[test]
    fn posters_are_picked_per_language_by_vote() {
        let images: sdks::tmdb::Images = serde_json::from_value(serde_json::json!({
            "posters": [
                { "file_path": "/en-low.jpg", "iso_639_1": "en", "vote_average": 5.0, "vote_count": 3 },
                { "file_path": "/en-high.jpg", "iso_639_1": "en", "vote_average": 5.6, "vote_count": 1 },
                { "file_path": "/es.jpg", "iso_639_1": "es", "vote_average": 9.0, "vote_count": 9 },
                { "file_path": "/none.jpg", "iso_639_1": null, "vote_average": 9.9, "vote_count": 9 }
            ]
        }))
        .unwrap();
        let lang = |l: &str| -> MetadataLanguage {
            l.parse()
                .unwrap()
        };
        assert_eq!(
            poster_in_language(Some(&images), &lang("en-gb")).as_deref(),
            Some("https://image.tmdb.org/t/p/w780/en-high.jpg")
        );
        assert_eq!(poster_in_language(Some(&images), &lang("fr")), None);
        assert_eq!(poster_in_language(None, &lang("en")), None);
    }

    #[test]
    fn detail_calls_request_images_in_active_languages() {
        let langs: BTreeSet<MetadataLanguage> = ["fr", "pt-br", "en"]
            .iter()
            .map(|l| {
                l.parse()
                    .unwrap()
            })
            .collect();
        assert_eq!(
            with_translation_image_languages(
                thumb_and_logo_languages(Some("es")),
                &langs
            ),
            "es,en,fr,pt,null"
        );
        assert_eq!(
            with_translation_image_languages(
                thumb_and_logo_languages(Some("es")),
                &BTreeSet::new()
            ),
            "es,en,null"
        );
    }

    #[tokio::test]
    async fn search_shows_user_language_titles_without_caching_them() {
        let tmdb = httpmock::MockServer::start();
        let translated = tmdb.mock(|when, then| {
            when.path("/search/movie")
                .query_param("language", "es");
            // TMDB falls back to the original title where it has no translation.
            then.status(200)
                .json_body(serde_json::json!({ "results": [
                    { "id": 478137, "title": "Kontroll", "original_title": "Kontroll", "original_language": "hu" },
                    { "id": 129, "title": "El viaje de Chihiro", "original_title": "千と千尋の神隠し" },
                    { "id": 1417, "title": "El laberinto del fauno", "original_title": "El laberinto del fauno", "original_language": "es" }
                ]}));
        });
        tmdb.mock(|when, then| {
            when.path("/search/movie")
                .query_param("language", "en");
            then.status(200)
                .json_body(serde_json::json!({ "results": [
                    { "id": 129, "title": "Spirited Away", "original_title": "千と千尋の神隠し" },
                    { "id": 478137, "title": "Control", "original_title": "Kontroll" },
                    { "id": 1417, "title": "Pan's Labyrinth", "original_title": "El laberinto del fauno" }
                ]}));
        });
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;
        let es: MetadataLanguage = "es"
            .parse()
            .unwrap();

        let titles = |results: Vec<db::Media>| -> Vec<String> {
            results
                .into_iter()
                .map(|m| m.title)
                .collect()
        };
        let results = ctx
            .addons
            .search(&db::MediaKind::Movie, "chihiro", 10, ctx, None, Some(&es))
            .await
            .unwrap();
        assert_eq!(
            titles(results),
            vec!["El viaje de Chihiro", "Control", "El laberinto del fauno"],
            "an original title is kept for a reader of the original language"
        );

        let cached = ctx
            .store
            .get::<db::Media>(
                common::stable_media_uuid(&db::MediaKind::Movie, "tmdb:129")
                    .to_string(),
            )
            .expect("stub cached");
        assert_eq!(
            cached.title, "Spirited Away",
            "shared cache holds server-language text"
        );

        let results = ctx
            .addons
            .search(&db::MediaKind::Movie, "chihiro", 10, ctx, None, None)
            .await
            .unwrap();
        assert_eq!(
            titles(results),
            vec!["Spirited Away", "Control", "Pan's Labyrinth"]
        );
        translated.assert_hits(1);
    }

    #[tokio::test]
    async fn slow_translated_search_does_not_hold_up_results() {
        let tmdb = httpmock::MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/search/movie")
                .query_param("language", "es");
            then.status(200)
                .delay(Duration::from_secs(10))
                .json_body(serde_json::json!({ "results": [
                    { "id": 129, "title": "El viaje de Chihiro", "original_title": "千と千尋の神隠し" }
                ]}));
        });
        tmdb.mock(|when, then| {
            when.path("/search/movie")
                .query_param("language", "en");
            then.status(200)
                .json_body(serde_json::json!({ "results": [
                    { "id": 129, "title": "Spirited Away", "original_title": "千と千尋の神隠し" }
                ]}));
        });
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;
        let es: MetadataLanguage = "es"
            .parse()
            .unwrap();

        let started = std::time::Instant::now();
        let results = ctx
            .addons
            .search(&db::MediaKind::Movie, "chihiro", 10, ctx, None, Some(&es))
            .await
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "{:?}",
            started.elapsed()
        );
        assert_eq!(results[0].title, "Spirited Away");
    }

    #[tokio::test]
    async fn seasons_and_episodes_get_text_from_one_call_per_season_and_language() {
        let tmdb = httpmock::MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/tv/1399");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": 1399,
                    "name": "Game of Thrones",
                    "seasons": [{ "id": 3624, "name": "Season 1", "season_number": 1 }]
                }));
        });
        let translated_season = tmdb.mock(|when, then| {
            when.path("/tv/1399/season/1")
                .query_param("language", "es");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": 3624, "name": "Temporada 1", "overview": "La primera temporada.",
                    "season_number": 1,
                    "episodes": [{ "id": 63056, "name": "Se acerca el invierno",
                                   "overview": "Episodio uno.", "episode_number": 1, "season_number": 1 }]
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/tv/1399/season/1")
                .query_param("language", "en");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": 3624, "name": "Season 1", "season_number": 1,
                    "episodes": [{ "id": 63056, "name": "Winter Is Coming",
                                   "overview": "Episode one.", "episode_number": 1, "season_number": 1 }]
                }));
        });
        let guard = ctx_with_tmdb(&tmdb).await;
        let ctx = &guard.0;
        set_user_language(ctx, "es").await;
        let config = db::Settings::get_config_or_default(&ctx.db).await;

        let series = Arc::new(db::Media {
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                tmdb: Some(1399),
                ..Default::default()
            },
            ..Default::default()
        });
        let season = db::Media {
            kind: db::MediaKind::Season,
            idx: Some(1),
            grandparent: Some(Arc::clone(&series)),
            ..Default::default()
        };
        let episode = db::Media {
            kind: db::MediaKind::Episode,
            idx: Some(1),
            parent_idx: Some(1),
            grandparent: Some(series),
            ..Default::default()
        };

        let es = |patch: db::Media| {
            patch
                .translations
                .into_iter()
                .map(|t| {
                    (
                        t.language
                            .to_string(),
                        t.title,
                        t.description,
                    )
                })
                .collect::<Vec<_>>()
        };
        let season_patch = fetch_tmdb_meta(&season, ctx, &config)
            .await
            .unwrap()
            .expect("season patch");
        assert_eq!(
            es(season_patch),
            vec![(
                "es".to_string(),
                Some("Temporada 1".to_string()),
                Some("La primera temporada.".to_string())
            )]
        );
        let episode_patch = fetch_tmdb_meta(&episode, ctx, &config)
            .await
            .unwrap()
            .expect("episode patch");
        assert_eq!(episode_patch.title, "Winter Is Coming");
        assert_eq!(
            es(episode_patch),
            vec![(
                "es".to_string(),
                Some("Se acerca el invierno".to_string()),
                Some("Episodio uno.".to_string())
            )]
        );
        translated_season.assert_hits(1);
    }
}
