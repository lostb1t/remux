use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use dashmap::DashMap;
use http::StatusCode;
use itertools::Itertools;
use remux_macros::{delete, get, patch, post};
use serde::Deserialize;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;
use tracing::debug;
use tracing::error;
use tracing::info;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

static PERSIST_LOCKS: OnceLock<DashMap<Uuid, Arc<Mutex<()>>>> = OnceLock::new();

fn persist_locks() -> &'static DashMap<Uuid, Arc<Mutex<()>>> {
    PERSIST_LOCKS.get_or_init(DashMap::new)
}

/// For each candidate ID: if not in DB, create/acquire its persist lock and persist if still
/// missing; if in DB but lock is held, wait for it. Returns true if a query retry is warranted.
async fn wait_for_persist(
    ids: &[Uuid],
    ctx: &crate::AppContext,
) -> anyhow::Result<bool> {
    for &id in ids {
        let in_db = db::Media::get_by_id(&ctx.db, &id).await?.is_some();
        if !in_db {
            let lock = persist_locks()
                .entry(id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            let _guard = lock.lock().await;
            if db::Media::get_by_id(&ctx.db, &id).await?.is_none() {
                ctx.search.persist(id, ctx).await.ok();
            }
            persist_locks().remove(&id);
            return Ok(true);
        } else if let Some(lock) = persist_locks().get(&id).map(|e| Arc::clone(&e)) {
            let _guard = lock.lock().await;
            return Ok(true);
        }
    }
    Ok(false)
}

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::auth;
use crate::errors::LogErr;
use crate::sdks;
use crate::utils::IntoVec;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};
use chrono::Datelike;
use chrono::Utc;
use sqlx::SqlitePool;

use super::{mock_items, stub_json};

pub struct ItemsQueryResult {
    pub items: Vec<api::BaseItemDto>,
    pub total_count: i64,
}

fn apply_permissions(item: &mut api::BaseItemDto, user: &db::User) {
    item.can_delete = Some(db::Media::can_delete(user));
}

impl ItemsQueryResult {
    pub fn with_permissions(mut self, session: &auth::AuthSession) -> Self {
        for item in &mut self.items {
            apply_permissions(item, &session.user);
        }
        self
    }
}

/// `GET /api/danmu/{item_id}/raw` — danmu not supported; return 404 so clients don't get SPA HTML.
#[get("/api/danmu/{item_id}/raw")]
pub async fn get_danmu_raw(
    _session: crate::db::auth::AuthSession,
    Path(_item_id): Path<String>,
) -> impl IntoResponse {
    StatusCode::NOT_FOUND
}

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    //trace!(?q, "get_items");

    let parent = if let Some(parent_id) = q.parent_id.clone() {
        db::Media::get_by_id(&state.ctx.db, &parent_id).await?
    } else {
        None
    };

    let search = q.search_term.clone();
    let skip = q.start_index.unwrap_or(0) as u32;

    // "local:" prefix bypasses AIO and falls through to the DB query path below,
    // enabling local title-contains search for any media kind (Genre, Studio, Person, …).
    if let Some(local_term) = search.as_deref().and_then(|s| s.strip_prefix("local:")) {
        q.search_term = Some(local_term.to_string());
    } else if search.is_some()
        || parent
            .clone()
            .map_or(false, |p| p.kind == db::MediaKind::Collection)
    {
        let types = q.get_requested_item_types();

        // Music (Audio / MusicAlbum / MusicArtist) — route to yt-dlp search.
        // Must check q.include_item_types directly because get_requested_item_types()
        // strips music kinds out.
        let raw_types = q.include_item_types.as_deref().unwrap_or(&[]);
        let cfg = db::Settings::get_config(&state.ctx.db).await?;
        let wants_tracks = cfg.search_tracks_remote.unwrap_or(true)
            && raw_types.iter().any(|t| matches!(t, api::MediaType::Audio));
        let wants_albums = cfg.search_albums_remote.unwrap_or(true)
            && raw_types
                .iter()
                .any(|t| matches!(t, api::MediaType::MusicAlbum));
        let wants_artists = cfg.search_artists_remote.unwrap_or(true)
            && raw_types
                .iter()
                .any(|t| matches!(t, api::MediaType::MusicArtist));
        let wants_music = wants_tracks || wants_albums || wants_artists;
        let wants_movies = cfg.search_movies_remote.unwrap_or(true)
            && types.contains(&api::MediaType::Movie);
        let wants_series = cfg.search_series_remote.unwrap_or(true)
            && types.contains(&api::MediaType::Series);
        let wants_video = wants_movies || wants_series;
        let wants_people = cfg.search_people_remote.unwrap_or(true)
            && raw_types
                .iter()
                .any(|t| matches!(t, api::MediaType::Person));

        if let Some(s) = search {
            let limit = q.limit.unwrap_or(800) as usize;
            let music_limit = limit.min(50);
            let mut all_items: Vec<api::BaseItemDto> = vec![];

            let search_start = std::time::Instant::now();
            let mut tracks_count = 0usize;
            let mut albums_count = 0usize;
            let mut artists_count = 0usize;
            let mut movies_count = 0usize;
            let mut series_count = 0usize;
            let mut people_count = 0usize;

            // Music: remote search or local DB fallback per type
            for (wants_remote, kind, api_type, count_ref) in [
                (
                    wants_tracks,
                    db::MediaKind::Track,
                    api::MediaType::Audio,
                    &mut tracks_count as &mut usize,
                ),
                (
                    wants_albums,
                    db::MediaKind::Album,
                    api::MediaType::MusicAlbum,
                    &mut albums_count,
                ),
                (
                    wants_artists,
                    db::MediaKind::Artist,
                    api::MediaType::MusicArtist,
                    &mut artists_count,
                ),
            ] {
                if !raw_types.iter().any(|t| *t == api_type) {
                    continue;
                }
                let item_limit = if matches!(kind, db::MediaKind::Track) {
                    music_limit
                } else {
                    music_limit.min(10)
                };
                if wants_remote {
                    match state
                        .ctx
                        .search
                        .search(&kind, &s, item_limit, &state.ctx)
                        .await
                    {
                        Ok(results) => {
                            *count_ref = results.len();
                            all_items
                                .extend(results.into_iter().map(api::db_media_to_item));
                        }
                        Err(e) => {
                            warn!(error = %e, term = %s, ?kind, "get_items: remote music search failed")
                        }
                    }
                } else {
                    let mut local_q = q.clone();
                    local_q.search_term = Some(s.clone());
                    local_q.include_item_types = Some(vec![api_type]);
                    local_q.parent_id = None;
                    local_q.start_index = None;
                    local_q.limit = Some(item_limit as u32);
                    let server_config =
                        crate::db::Settings::get_config(&state.ctx.db).await.ok();
                    match db::Media::get_by_jellyfin_filter(
                        &state.ctx.db,
                        &local_q,
                        false,
                        Some(&session.user),
                        server_config.as_ref(),
                        None,
                    )
                    .await
                    {
                        Ok(r) => {
                            *count_ref = r.records.len();
                            all_items.extend(
                                r.records.into_iter().map(api::db_media_to_item),
                            );
                        }
                        Err(e) => {
                            warn!(error = %e, term = %s, ?kind, "get_items: local music search failed")
                        }
                    }
                }
            }

            // Video: remote search or local DB fallback per type
            if !types.iter().all(|t| matches!(t, api::MediaType::Episode)) {
                for (wants_remote, kind, api_type, count_ref) in [
                    (
                        wants_movies,
                        db::MediaKind::Movie,
                        api::MediaType::Movie,
                        &mut movies_count as &mut usize,
                    ),
                    (
                        wants_series,
                        db::MediaKind::Series,
                        api::MediaType::Series,
                        &mut series_count,
                    ),
                ] {
                    if !types.contains(&api_type) {
                        continue;
                    }
                    if wants_remote {
                        match state
                            .ctx
                            .search
                            .search(&kind, &s, limit, &state.ctx)
                            .await
                        {
                            Ok(results) => {
                                let items: Vec<_> = results
                                    .into_iter()
                                    .map(api::db_media_to_item)
                                    .filter(|item| {
                                        types.contains(&item.type_)
                                            && q.media_types
                                                .as_ref()
                                                .map_or(true, |mt| {
                                                    mt.contains(&item.media_type)
                                                })
                                    })
                                    .collect();
                                *count_ref = items.len();
                                all_items.extend(items);
                            }
                            Err(e) => {
                                warn!(error = %e, ?kind, "get_items: remote video search failed")
                            }
                        }
                    } else {
                        let mut local_q = q.clone();
                        local_q.search_term = Some(s.clone());
                        local_q.include_item_types = Some(vec![api_type]);
                        local_q.parent_id = None;
                        local_q.start_index = None;
                        local_q.limit = Some(limit as u32);
                        let server_config =
                            crate::db::Settings::get_config(&state.ctx.db).await.ok();
                        match db::Media::get_by_jellyfin_filter(
                            &state.ctx.db,
                            &local_q,
                            false,
                            Some(&session.user),
                            server_config.as_ref(),
                            None,
                        )
                        .await
                        {
                            Ok(r) => {
                                *count_ref = r.records.len();
                                all_items.extend(
                                    r.records.into_iter().map(api::db_media_to_item),
                                );
                            }
                            Err(e) => {
                                warn!(error = %e, ?kind, "get_items: local video search failed")
                            }
                        }
                    }
                }
            }

            // People: remote TMDB search.
            if wants_people {
                match state
                    .ctx
                    .search
                    .search(&db::MediaKind::Person, &s, limit.min(20), &state.ctx)
                    .await
                {
                    Ok(results) => {
                        people_count = results.len();
                        all_items
                            .extend(results.into_iter().map(api::db_media_to_item));
                    }
                    Err(e) => warn!(error = %e, "get_items: people search failed"),
                }
            }

            let mut counts = vec![];
            if raw_types.iter().any(|t| matches!(t, api::MediaType::Audio)) {
                counts.push(format!("tracks={tracks_count}"));
            }
            if raw_types
                .iter()
                .any(|t| matches!(t, api::MediaType::MusicAlbum))
            {
                counts.push(format!("albums={albums_count}"));
            }
            if raw_types
                .iter()
                .any(|t| matches!(t, api::MediaType::MusicArtist))
            {
                counts.push(format!("artists={artists_count}"));
            }
            if types.contains(&api::MediaType::Movie) {
                counts.push(format!("movies={movies_count}"));
            }
            if types.contains(&api::MediaType::Series) {
                counts.push(format!("series={series_count}"));
            }
            if wants_people {
                counts.push(format!("people={people_count}"));
            }
            debug!(
                query = %s,
                counts = %counts.join(" "),
                elapsed_ms = search_start.elapsed().as_millis(),
                "search"
            );

            let total_count = all_items.len() as i64;
            let paged_items = all_items
                .into_iter()
                .skip(skip as usize)
                .take(limit)
                .collect();

            return Ok(ItemsQueryResult {
                total_count,
                items: paged_items,
            });
        }
    }

    // if q.filters.is_some() {
    //     return Ok(ItemsQueryResult {
    //         items: vec![],
    //         total_count: 0,
    //     });
    // }

    //let manifest = aio.get_manifest().await?;

    if let Some(parent) = &parent {
        if parent.id == db::collection_uuid() {
            // Virtual collections root — clear parent_id (collections have no parent in DB)
            // and ensure we only return non-promoted collections. Promoted collections
            // are libraries and should not be listed in the collections view.
            q.parent_id = None;
            q.promoted = Some(false);
            if q.include_item_types.is_none() {
                q.include_item_types = Some(vec![api::MediaType::BoxSet]);
            }
        }

        // collection browse
        if parent.kind == db::MediaKind::Collection {
            // All collection types: items float freely (no parent_id constraint).
            q.parent_id = None;

            let media_kind_filter =
                if let Some(kind) = parent.collection_media_kind.clone() {
                    match kind {
                        db::CollectionMediaKind::Movie => vec![db::MediaKind::Movie],
                        db::CollectionMediaKind::Series => vec![db::MediaKind::Series],
                        db::CollectionMediaKind::Music => vec![
                            db::MediaKind::Track,
                            db::MediaKind::Album,
                            db::MediaKind::Artist,
                        ],
                    }
                } else {
                    vec![db::MediaKind::Movie, db::MediaKind::Series]
                };

            q.include_item_types = Some({
                let collection_types: Vec<api::MediaType> = media_kind_filter
                    .iter()
                    .map(|k| api::db_media_kind_to_type(k.clone()))
                    .collect();
                // Respect the client's IncludeItemTypes filter if provided,
                // but constrain it to what this collection actually holds.
                if let Some(requested) = &q.include_item_types {
                    let intersection: Vec<_> = requested
                        .iter()
                        .filter(|t| collection_types.contains(t))
                        .cloned()
                        .collect();
                    if intersection.is_empty() {
                        collection_types
                    } else {
                        intersection
                    }
                } else {
                    collection_types
                }
            });

            if q.limit.is_none() {
                q.limit = Some(250);
            }
            q.user_id = Some(session.user.id.clone());

            // Smart collection: extract stored filter rules so they are applied
            // alongside the Jellyfin query (sort, pagination, user-state, etc.).
            let smart_filter =
                if parent.collection_kind == Some(db::CollectionKind::Smart) {
                    parent.parse_smart_filter()
                } else {
                    None
                };

            let server_config =
                crate::db::Settings::get_config(&state.ctx.db).await.ok();
            let result = db::Media::get_by_jellyfin_filter(
                &state.ctx.db,
                &q,
                true,
                Some(&session.user),
                server_config.as_ref(),
                smart_filter,
            )
            .await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result
                    .records
                    .into_iter()
                    .map(api::db_media_to_item)
                    .collect(),
            });
        }

        //  }
    }
    // Map season_id → parent_id if parent_id not already set
    if q.season_id.is_some() && q.parent_id.is_none() {
        q.parent_id = q.season_id.take();
    }

    // Always provide user_id so user-state filters work
    if q.user_id.is_none() {
        q.user_id = Some(session.user.id);
    }

    let want_total = q.enable_total_record_count.unwrap_or(true);
    let server_config = crate::db::Settings::get_config(&state.ctx.db).await.ok();
    //trace!(?q, "get_items");
    let mut result = db::Media::get_by_jellyfin_filter(
        &state.ctx.db,
        &q,
        want_total,
        Some(&session.user),
        server_config.as_ref(),
        None,
    )
    .await?;

    // If result is empty, a parent/artist tree may still be mid-persist. Collect all candidate
    // IDs from the query and wait on whichever has (or needs) a persist lock, then retry once.
    if result.records.is_empty() {
        let candidates: Vec<Uuid> = q
            .parent_id
            .iter()
            .chain(q.artist_ids.as_deref().unwrap_or(&[]))
            .chain(q.album_artist_ids.as_deref().unwrap_or(&[]))
            .chain(q.contributing_artist_ids.as_deref().unwrap_or(&[]))
            .copied()
            .collect();

        if wait_for_persist(&candidates, &state.ctx).await? {
            result = db::Media::get_by_jellyfin_filter(
                &state.ctx.db,
                &q,
                want_total,
                Some(&session.user),
                server_config.as_ref(),
                None,
            )
            .await?;
        }
    }

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
            let media = item(state, session, ids[0], q.fields.as_deref()).await?;
            if let Some(media) = media {
                return Ok(ItemsQueryResult {
                    items: vec![media],
                    total_count: 1,
                });
            }
        }
    }

    Ok(ItemsQueryResult {
        items: result
            .records
            .into_iter()
            .map(api::db_media_to_item)
            .collect(),
        total_count: result.total_count as i64,
    })
}

#[get("/items/latest")]
pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state.clone(), session.clone(), q, false)
        .await?
        .with_permissions(&session);
    Ok(Json::<Vec<api::BaseItemDto>>(items.items))
}

#[get("/items")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state.clone(), session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

/// Return the virtual root folder
#[get("/items/root")]
pub async fn items_root(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDto {
        id: db::collection_uuid(),
        name: Some("Media Library".to_string()),
        type_: api::MediaType::CollectionFolder,
        is_folder: true,
        ..Default::default()
    }))
}

/// Get ancestor items walking up the parent chain
#[get("/items/{id}/ancestors")]
pub async fn items_ancestors(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let ancestors = db::Media::get_ancestors(&state.ctx.db, &id).await?;
    Ok(Json(
        ancestors
            .into_iter()
            .map(api::db_media_to_item)
            .collect::<Vec<_>>(),
    ))
}

/// Delete a media item
#[delete("/items/{id}")]
pub async fn delete_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    db::Media::delete(&state.ctx.db, &id).await?;
    let _ = state.ctx.ws_tx.send(crate::ws::WsEvent::LibraryChanged);
    Ok(StatusCode::NO_CONTENT)
}

/// Controls what happens during an item refresh.
#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
pub enum MetadataRefreshMode {
    /// Re-fetch streams from AIO for the item (or its parent if a Source).
    #[default]
    Default,
    /// Run the full metadata provider pipeline.
    #[serde(other)]
    Full,
}

/// Refresh a single item — behaviour depends on `MetadataRefreshMode`:
/// - `Default`      → re-fetch streams from AIO for the item (or its parent if a Source).
/// - anything else  → run the full metadata provider pipeline.
#[derive(Debug, Deserialize, Default)]
pub struct RefreshItemQuery {
    #[serde(rename = "MetadataRefreshMode", default)]
    pub metadata_refresh_mode: MetadataRefreshMode,
    #[serde(rename = "ReplaceAllMetadata", default)]
    pub replace_all_metadata: bool,
}

#[post("/items/{id}/refresh")]
pub async fn refresh_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<RefreshItemQuery>,
) -> Result<StatusCode> {
    let mut media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Item not found")?;

    // If the requested item is a Source (stream), navigate to its parent.
    if media.kind == db::MediaKind::Source {
        let parent_id = media
            .parent_id
            .context_not_found("Not Found", "Source has no parent item")?;
        media = db::Media::get_by_id(&state.ctx.db, &parent_id)
            .await?
            .context_not_found("Not Found", "Parent item not found")?;
    }

    if q.metadata_refresh_mode == MetadataRefreshMode::Default {
        // Force-refresh streams by clearing the timestamp first.
        sqlx::query("UPDATE media SET streams_refreshed_at = NULL WHERE id = ?")
            .bind(media.id)
            .execute(&state.ctx.db)
            .await
            .ok();

        state
            .ctx
            .streams
            .refresh_sources(&media, &state.ctx)
            .await
            .inspect_err(|e| error!("Could not refresh streams: {e:#}"));

        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
            warm_subtitle_cache(&state.ctx.db, &media);
        }
    } else {
        // Refresh metadata via the full provider pipeline.
        let service = crate::providers::MetaProviderService::default();
        let force_refresh = q.replace_all_metadata;
        service
            .process(vec![media], &state.ctx, force_refresh, true)
            .await?;
    }

    let _ = state.ctx.ws_tx.send(crate::ws::WsEvent::LibraryChanged);
    Ok(StatusCode::NO_CONTENT)
}

/// Get filter options (genres + tags) for the modern /Items/Filters2 endpoint
#[get("/items/filters2")]
pub async fn items_filters2(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| db::MediaKind::try_from(t).ok())
        .collect();
    let genres = db::Media::get_genres(&state.ctx.db, &kinds).await?;
    let tag_rows = sqlx::query("SELECT DISTINCT tag FROM media_tags ORDER BY tag")
        .fetch_all(&state.ctx.db)
        .await?;
    Ok(Json(api::QueryFilters {
        genres: Some(
            genres
                .into_iter()
                .map(|g| api::NameIdPair {
                    id: g.id,
                    name: g.title,
                })
                .collect(),
        ),
        tags: Some(
            tag_rows
                .iter()
                .map(|r| {
                    use sqlx::Row;
                    r.get::<String, _>(0)
                })
                .collect(),
        ),
    }))
}

/// List distinct tags, optionally filtered by search_term (substring match)
#[get("/items/tags")]
pub async fn items_tags(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let tags: Vec<String> = match q.search_term.as_deref() {
        Some(s) if !s.is_empty() => {
            let pattern = format!("%{}%", s.to_lowercase());
            sqlx::query(
                "SELECT DISTINCT tag FROM media_tags WHERE lower(tag) LIKE ? ORDER BY tag LIMIT 25",
            )
            .bind(&pattern)
            .fetch_all(&state.ctx.db)
            .await?
            .iter()
            .map(|r| {
                use sqlx::Row;
                r.get::<String, _>(0)
            })
            .collect()
        }
        _ => sqlx::query("SELECT DISTINCT tag FROM media_tags ORDER BY tag LIMIT 50")
            .fetch_all(&state.ctx.db)
            .await?
            .iter()
            .map(|r| {
                use sqlx::Row;
                r.get::<String, _>(0)
            })
            .collect(),
    };
    Ok(Json(tags))
}

/// List distinct certifications, optionally filtered by search_term
#[get("/items/certifications")]
pub async fn items_certifications(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let values: Vec<String> = match q.search_term.as_deref() {
        Some(s) if !s.is_empty() => {
            let pattern = format!("%{}%", s.to_lowercase());
            sqlx::query(
                "SELECT DISTINCT certification FROM media \
                 WHERE certification IS NOT NULL AND lower(certification) LIKE ? \
                 ORDER BY certification LIMIT 25",
            )
            .bind(&pattern)
            .fetch_all(&state.ctx.db)
            .await?
            .iter()
            .map(|r| {
                use sqlx::Row;
                r.get::<String, _>(0)
            })
            .collect()
        }
        _ => sqlx::query(
            "SELECT DISTINCT certification FROM media \
                 WHERE certification IS NOT NULL ORDER BY certification LIMIT 50",
        )
        .fetch_all(&state.ctx.db)
        .await?
        .iter()
        .map(|r| {
            use sqlx::Row;
            r.get::<String, _>(0)
        })
        .collect(),
    };
    Ok(Json(values))
}

/// Trigger a full library refresh (re-imports all promoted catalogs)
#[post("/library/refresh")]
pub async fn library_refresh(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<StatusCode> {
    let catalogs = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records;
    for cat in catalogs {
        let key = crate::tasks::CatalogItemImportTask::task_key(cat.id);
        let _ = state.tasks.run_task(&key).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Stubs — Jellyfin clients call these; we return empty lists so they don't 404

#[get("/items/{id}/localtrailers")]
pub async fn items_local_trailers(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<api::BaseItemDto>::new()))
}

#[get("/items/{id}/specialfeatures")]
pub async fn items_special_features(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<api::BaseItemDto>::new()))
}

#[get("/items/{id}/externalidinfos")]
pub async fn items_external_id_infos(
    _state: State<AppState>,
    _session: auth::AdminSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<api::ExternalIdInfo>::new()))
}

#[get("/items/{id}/themevideos")]
pub async fn items_theme_videos(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

#[get("/items/{id}/themesongs")]
pub async fn items_theme_songs(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

#[get("/items/{id}/remoteimages")]
pub async fn items_remote_images(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::RemoteImageResult {
        images: Some(vec![]),
        total_record_count: 0,
        providers: Some(vec![]),
    }))
}

#[get("/items/{id}/remoteimages/providers")]
pub async fn items_remote_images_providers(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<String>::new()))
}

/// Get item counts
#[get("/items/counts")]
pub async fn items_counts(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let (
        movie_count,
        series_count,
        episode_count,
        song_count,
        album_count,
        artist_count,
    ) = tokio::try_join!(
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Movie),
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Series),
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Episode),
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Track),
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Album),
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Artist),
    )?;
    let (
        movie_count,
        series_count,
        episode_count,
        song_count,
        album_count,
        artist_count,
    ) = (
        movie_count as i32,
        series_count as i32,
        episode_count as i32,
        song_count as i32,
        album_count as i32,
        artist_count as i32,
    );
    let item_counts = api::ItemCounts {
        movie_count,
        series_count,
        episode_count,
        song_count,
        album_count,
        artist_count,
        item_count: movie_count
            + series_count
            + episode_count
            + song_count
            + album_count,
        ..Default::default()
    };

    Ok(Json(item_counts))
}

pub async fn item(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
    fields: Option<&[api::ItemFields]>,
) -> Result<Option<api::BaseItemDto>> {
    let want_sources = fields
        .map(|f| f.contains(&api::ItemFields::MediaSources))
        .unwrap_or(true);
    let mut media = match db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            id: Some(vec![id]),
            include_user_state: true,
            include_child_count: true,
            user_id: Some(session.user.id),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    {
        Some(m) => m,
        None => {
            // Two concurrent requests for the same id (Jellyfin web UI bug):
            // serialise here so only one triggers the expensive persist.
            let lock = persist_locks()
                .entry(id)
                .or_insert_with(|| Arc::new(Mutex::new(())))
                .clone();
            let _guard = lock.lock().await;

            // Re-check under lock — first waiter may have just saved it.
            match db::Media::get_by_filter(
                &state.ctx.db,
                &db::MediaFilter {
                    id: Some(vec![id]),
                    include_user_state: true,
                    include_child_count: true,
                    user_id: Some(session.user.id),
                    ..Default::default()
                },
            )
            .await?
            .records
            .into_iter()
            .next()
            {
                Some(m) => m,
                None => match state.ctx.search.persist(id, &state.ctx).await? {
                    Some(m) => {
                        persist_locks().remove(&id);
                        m
                    }
                    None => {
                        persist_locks().remove(&id);
                        return Ok(None);
                    }
                },
            }
        }
    };

    let need_refresh = media.refreshed_at.is_none()
        && matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series);
    let needs_sources = want_sources
        && matches!(
            media.kind,
            db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
        );

    tokio::join!(
        async {
            if need_refresh {
                let service = crate::providers::MetaProviderService::default();
                service
                    .process(vec![media.clone()], &state.ctx, false, true)
                    .await
                    .log_err("failed to refresh metadata")
            } else {
                Ok(vec![])
            }
        },
        async {
            if needs_sources {
                if media.kind == db::MediaKind::Movie
                    || media.kind == db::MediaKind::Episode
                {
                    let db = state.ctx.db.clone();
                    if let Ok(aio) = crate::aio::AioService::from_settings(&db).await {
                        warm_subtitle_cache(&db, &media);
                    }
                }
                state
                    .ctx
                    .streams
                    .refresh_sources(&media, &state.ctx)
                    .await
                    .log_err("failed to refresh sources");
            }
            Ok::<(), anyhow::Error>(())
        }
    );

    if media.kind == db::MediaKind::Source {
        media.sources = Some(vec![media.clone()]);
    } else if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
        media.streams(&state.ctx.db).await?;
        media.user_state(&state.ctx.db, &session.user).await?;
    } else if media.kind == db::MediaKind::Track {
        media.streams(&state.ctx.db).await?;
        media.user_state(&state.ctx.db, &session.user).await?;
    }
    // info!("Seasons length: {:?}", media.seasons(&state.ctx.db).await?.len());
    media.load_relations(&state.ctx.db).await?;
    let mut base_item = api::db_media_to_item(media.clone());

    // For tracks, wrap the Source row(s) as HLS-transcoded MediaSources.
    // CDN URLs are IP-locked to the server; the client must go through the HLS pipeline.
    if media.kind == db::MediaKind::Track {
        let transcoding_url = format!(
            "/videos/{}/master.m3u8?MediaSourceId={}&VideoCodec=copy&AudioCodec=aac&ApiKey={}",
            media.id, media.id, session.device.access_token
        );
        let sources = media.sources.as_deref().unwrap_or(&[]);
        let mut media_streams: Vec<api::MediaStream> = sources
            .first()
            .and_then(|s| s.probe_data.as_ref())
            .map(|p| p.media_streams.clone())
            .unwrap_or_else(|| {
                vec![api::MediaStream {
                    index: 0,
                    type_: Some(api::MediaStreamType::Audio),
                    codec: Some("aac".to_string()),
                    channels: Some(2),
                    is_default: Some(true),
                    display_title: Some("Audio".to_string()),
                    ..Default::default()
                }]
            });

        let mut source = api::MediaSourceInfo {
            id: media.id,
            e_tag: media.id,
            name: Some(media.title.clone()),
            protocol: "Http".to_string(),
            is_remote: true,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: true,
            transcoding_url: Some(transcoding_url),
            transcoding_sub_protocol: "hls".to_string(),
            transcoding_container: Some("ts".to_string()),
            run_time_ticks: media.runtime.map(|s| s * 10_000_000),
            media_streams,
            ..Default::default()
        };
        api::inject_lyric_stream(&mut source);
        base_item.media_sources = Some(vec![source]);
    }

    if media.kind == db::MediaKind::Episode {
        if let Some(sid) = media.series_id {
            if let Ok(Some(s)) = db::Media::get_by_id(&state.ctx.db, &sid).await {
                base_item.series_name = Some(s.title);
                base_item.series_id = Some(s.id);
            }
        }
        if let Some(pid) = media.parent_id {
            if let Ok(Some(s)) = db::Media::get_by_id(&state.ctx.db, &pid).await {
                base_item.season_name = Some(s.title);
                base_item.season_id = Some(s.id);
            }
        }
    } else if media.kind == db::MediaKind::Season {
        if let Some(pid) = media.parent_id {
            if let Ok(Some(s)) = db::Media::get_by_id(&state.ctx.db, &pid).await {
                base_item.series_name = Some(s.title);
                base_item.series_id = Some(s.id);
            }
        }
    }
    if media.sources.as_ref().is_none_or(|s| s.is_empty()) {
        base_item.location_type = Some("Virtual".to_string());
        base_item.path = None;
        base_item.can_download = Some(false);
    }

    let enable_subtitles_detail = crate::db::Settings::get_config(&state.ctx.db)
        .await
        .ok()
        .and_then(|c| c.enable_subtitles_detail)
        .unwrap_or(true);

    if enable_subtitles_detail {
        if let Some(ref mut sources) = base_item.media_sources {
            if !sources.is_empty() {
                super::playback::inject_external_subtitles(
                    &state.ctx.db,
                    &media,
                    sources,
                    media.id,
                    &session.device.access_token,
                )
                .await;
            }
        }
    }

    apply_permissions(&mut base_item, &session.user);
    Ok(Some(base_item))
}

/// Jellyfin web requests `/Items/livetv` (literal string) when navigating to
/// the Live TV section — handle it before the `{id}` UUID route.
#[get("/items/livetv")]
pub async fn items_livetv(_session: auth::AuthSession) -> Result<impl IntoResponse> {
    Ok(Json(super::shows::livetv_view_item()))
}

#[get("/items/{id}")]
pub async fn items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    return Ok(
        Json(item(state, session, id, q.fields.as_deref()).await?).into_response()
    );
}

#[get("/items/suggestions")]
pub async fn items_suggestions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    //let b = state.tmdb.movie_popular_list().send().await.unwrap()
    //.into_inner()
    //.results
    //.map(|c| {
    //  api::BaseItemDto {
    //     name: c.title,
    //     ..Default::default()
    //   }
    //}
    //);
    //let tmdb_items = state.tmdb.movie_now_playing().send().await;
    Ok(Json(api::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/persons")]
pub async fn persons(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.include_item_types = Some(vec![api::MediaType::Person]);
    let items = get_items(state.clone(), session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);
    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/items/filters")]
pub async fn items_filters(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| db::MediaKind::try_from(t).ok())
        .collect();

    let genres = db::Media::get_genres(&state.ctx.db, &kinds).await?;
    let years = db::Media::get_distinct_years(&state.ctx.db, &kinds).await?;

    Ok(Json(api::QueryFiltersLegacy {
        genres: Some(genres.into_iter().map(|g| g.title).collect()),
        years: Some(years),
        ..Default::default()
    }))
}

#[get("/library/mediafolders")]
pub async fn library_mediafolders(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let items = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|x| api::db_media_to_item(x))
    .collect::<Vec<_>>();

    let total = items.len() as i64;
    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        ..Default::default()
    }))
}

#[get("/library/virtualfolders")]
pub async fn library_virtualfolders(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let folders = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(media_to_virtual_folder)
    .collect::<Vec<_>>();

    Ok(Json(folders))
}

fn media_to_virtual_folder(m: db::Media) -> api::VirtualFolderInfo {
    let collection_type = m
        .collection_media_kind
        .clone()
        .map(api::db_media_kind_to_collection_type);
    api::VirtualFolderInfo {
        name: Some(m.title.clone()),
        item_id: Some(m.id.to_string()),
        collection_type,
        collection_kind: m.collection_kind.as_ref().map(|k| k.to_string()),
        promoted: Some(m.is_promoted()),
        collection_max_items: m.collection_max_items,
        ..Default::default()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct VirtualFolderRequest {
    name: String,
    collection_type: Option<String>,
    collection_kind: Option<String>,
    promoted: Option<bool>,
}

#[post("/library/virtualfolders")]
pub async fn create_virtual_folder(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<VirtualFolderRequest>,
) -> Result<Json<api::VirtualFolderInfo>> {
    let collection_media_kind = payload
        .collection_type
        .as_deref()
        .and_then(|s| parse_collection_type(s));

    let collection_kind = payload
        .collection_kind
        .as_deref()
        .and_then(|s| db::CollectionKind::try_from(s).ok())
        .unwrap_or(db::CollectionKind::Smart);

    let promoted = payload.promoted.unwrap_or(false);

    let mut media = db::Media {
        title: payload.name,
        kind: db::MediaKind::Collection,
        collection_kind: Some(collection_kind.clone()),
        collection_media_kind,
        promoted,
        ..Default::default()
    };

    media.save(&state.ctx.db).await?;

    Ok(Json(media_to_virtual_folder(media)))
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateVirtualFolderRequest {
    id: uuid::Uuid,
    name: String,
    collection_type: Option<String>,
    collection_kind: Option<String>,
    promoted: Option<bool>,
    collection_max_items: Option<i64>,
}

#[post("/library/virtualfolders/LibraryOptions")]
pub async fn update_virtual_folder(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<UpdateVirtualFolderRequest>,
) -> Result<StatusCode> {
    let media = db::Media::get_by_id(&state.ctx.db, &payload.id)
        .await?
        .context_not_found("Not Found", "Collection not found")?;

    if media.kind != db::MediaKind::Collection {
        return Err(anyhow::anyhow!("not a collection"))
            .context_bad_request("Bad Request", "Item is not a collection");
    }

    let collection_media_kind = payload
        .collection_type
        .as_deref()
        .and_then(|s| parse_collection_type(s));

    let collection_kind = payload
        .collection_kind
        .as_deref()
        .and_then(|s| db::CollectionKind::try_from(s).ok());

    let promoted = payload.promoted.unwrap_or(false);
    let updated_at = Utc::now().naive_utc();

    sqlx::query(
        "UPDATE media SET title = $1, promoted = $2, collection_media_kind = $3, collection_kind = $4, collection_max_items = $5, updated_at = $6 WHERE id = $7",
    )
    .bind(&payload.name)
    .bind(promoted)
    .bind(collection_media_kind.as_ref().map(|k| k.to_string()))
    .bind(collection_kind.as_ref().map(|k| k.to_string()))
    .bind(payload.collection_max_items)
    .bind(updated_at)
    .bind(payload.id)
    .execute(&state.ctx.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
struct DeleteVirtualFolderQuery {
    name: String,
}

#[delete("/library/virtualfolders")]
pub async fn delete_virtual_folder(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Query(q): Query<DeleteVirtualFolderQuery>,
) -> Result<StatusCode> {
    let result = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Collection]),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .find(|m| m.title == q.name);

    let media = result.context_not_found("Not Found", "Collection not found")?;

    db::Media::delete(&state.ctx.db, &media.id).await?;

    Ok(StatusCode::NO_CONTENT)
}

#[get("/aio/catalogs")]
pub async fn aio_catalogs(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let aio = crate::aio::AioService::from_settings(&state.ctx.db)
        .await
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?;

    let manifest = aio.get_manifest().await?;

    // Look up existing catalog media items to merge enabled/max_items
    let db_catalogs = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            ..Default::default()
        },
    )
    .await?
    .records;

    let catalogs: Vec<api::AioCatalogInfo> = manifest
        .catalogs
        .into_iter()
        .filter(|c| !c.id.contains("search"))
        .map(|c| {
            let aio_id = format!("{}:{}", c.kind, c.id);
            let namespaced_id = format!("aio:{}", aio_id);
            let db_cat = db_catalogs
                .iter()
                .find(|d| d.media_id.as_deref() == Some(&namespaced_id));
            api::AioCatalogInfo {
                aio_id,
                name: c.name,
                enabled: Some(db_cat.map(|d| d.is_promoted()).unwrap_or(false)),
                max_items: db_cat.and_then(|d| d.collection_max_items),
                media_id: db_cat.map(|d| d.id.to_string()),
            }
        })
        .collect();
    Ok(Json(catalogs))
}

// Anfiteatro/Gelato compatibility aliases.
#[get("/gelato/catalogs")]
pub async fn gelato_catalogs(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    aio_catalogs(State(state), session).await
}

#[get("/gelato/subtitles/{id}")]
pub async fn gelato_subtitles(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    // Source entries can sit under an episode/movie; normalize to the parent
    // media item before resolving subtitles.
    let Some(mut media) = db::Media::get_by_id(&state.ctx.db, &id).await? else {
        return Ok(Json(Vec::<sdks::aio::Subtitle>::new()));
    };

    if media.kind == db::MediaKind::Source {
        if let Some(parent) = media.parent(&state.ctx.db).await? {
            media = parent;
        }
    }

    if !matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
        return Ok(Json(Vec::<sdks::aio::Subtitle>::new()));
    }

    let Ok(aio) = crate::aio::AioService::from_settings(&state.ctx.db).await else {
        return Ok(Json(Vec::<sdks::aio::Subtitle>::new()));
    };

    let subtitles = media.get_subtitles(&aio).await.unwrap_or_default();
    Ok(Json(subtitles))
}

fn parse_collection_type(s: &str) -> Option<db::CollectionMediaKind> {
    match s {
        "movies" => Some(db::CollectionMediaKind::Movie),
        "tvshows" => Some(db::CollectionMediaKind::Series),
        "music" => Some(db::CollectionMediaKind::Music),
        _ => None,
    }
}

#[get("/genres")]
pub async fn genres(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let related_kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .filter_map(|t| db::MediaKind::try_from(t).ok())
        .collect();

    let genres = db::Media::get_genres(&state.ctx.db, &related_kinds).await?;
    let total = genres.len() as i64;

    Ok(Json(api::BaseItemDtoQueryResult {
        items: genres.into_iter().map(api::db_media_to_item).collect(),
        total_record_count: total,
        start_index: q.start_index.unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/items/{id}/metadataeditor")]
pub async fn items_metadata_editor(
    State(_state): State<AppState>,
    _session: auth::AdminSession,
    Path(_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::MetadataEditorInfo::default()))
}

#[get("/items/{id}/similar")]
pub async fn items_similar(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/items/{id}/thememedia")]
pub async fn items_thememedia(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    stub_json(State(state)).await
}

#[get("/channels")]
pub async fn channels(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

async fn set_tags(db: &SqlitePool, id: Uuid, tags: &[String]) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM media_tags WHERE media_id = ?")
        .bind(id)
        .execute(db)
        .await?;
    for tag in tags {
        sqlx::query("INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)")
            .bind(id)
            .bind(tag)
            .execute(db)
            .await?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemRequest {
    tags: Option<Vec<String>>,
}

#[post("/items/{id}")]
pub async fn update_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateItemRequest>,
) -> Result<StatusCode> {
    if let Some(tags) = &payload.tags {
        set_tags(&state.ctx.db, id, tags)
            .await
            .context_bad_request("Bad Request", "Failed to update tags")?;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PatchItemRequest {
    name: Option<String>,
    collection_type: Option<String>,
    collection_kind: Option<String>,
    smart_filter: Option<api::CollectionFilter>,
    promoted: Option<bool>,
    tags: Option<Vec<String>>,
    digital_released_at: Option<chrono::DateTime<chrono::Utc>>,
}

#[patch("/items/{id}")]
pub async fn patch_item(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<PatchItemRequest>,
) -> Result<StatusCode> {
    let updated_at = Utc::now().naive_utc();
    let mut qb = sqlx::QueryBuilder::new("UPDATE media SET updated_at = ");
    qb.push_bind(updated_at);

    if let Some(name) = &payload.name {
        qb.push(", title = ").push_bind(name);
    }
    if let Some(ct) = &payload.collection_type {
        let media_kind = parse_collection_type(ct);
        qb.push(", collection_media_kind = ")
            .push_bind(media_kind.as_ref().map(|k| k.to_string()));
    }
    if let Some(ck) = &payload.collection_kind {
        qb.push(", collection_kind = ").push_bind(ck);
    }
    if let Some(sf) = &payload.smart_filter {
        qb.push(", collection_smart_filter = ")
            .push_bind(sqlx::types::Json(sf));
    }
    if let Some(prm) = payload.promoted {
        qb.push(", promoted = ")
            .push_bind(if prm { 1i64 } else { 0i64 });
    }
    if let Some(dra) = payload.digital_released_at {
        qb.push(", digital_released_at = ")
            .push_bind(dra.naive_utc());
    }

    qb.push(" WHERE id = ").push_bind(id);
    qb.build().execute(&state.ctx.db).await?;

    if let Some(tags) = &payload.tags {
        set_tags(&state.ctx.db, id, tags)
            .await
            .context_bad_request("Bad Request", "Failed to update tags")?;
    }

    Ok(StatusCode::NO_CONTENT)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateCatalogSettingsRequest {
    enabled: bool,
    max_items: Option<i64>,
    /// Used to set the title when creating a new catalog media item
    name: Option<String>,
}

#[post("/aio/catalogs/{aio_id}")]
pub async fn update_catalog_settings(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(aio_id): Path<String>,
    Json(payload): Json<UpdateCatalogSettingsRequest>,
) -> Result<StatusCode> {
    let promoted = payload.enabled;
    let now = Utc::now().naive_utc();
    let namespaced_id = format!("aio:{}", aio_id);

    let existing = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            media_id: Some(namespaced_id.clone()),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next();

    let catalog_id;
    if let Some(cat) = existing {
        catalog_id = cat.id;
        sqlx::query(
            "UPDATE media SET promoted = $1, collection_max_items = $2, updated_at = $3 WHERE id = $4",
        )
        .bind(promoted)
        .bind(payload.max_items)
        .bind(now)
        .bind(catalog_id)
        .execute(&state.ctx.db)
        .await?;
    } else {
        let title = payload
            .name
            .clone()
            .unwrap_or_else(|| aio_id.clone())
            .trim()
            .to_string();
        let mut media = db::Media {
            kind: db::MediaKind::Catalog,
            title,
            media_id: Some(namespaced_id),
            promoted,
            collection_max_items: payload.max_items,
            ..Default::default()
        };
        media.save(&state.ctx.db).await?;
        catalog_id = media.id;
    }

    // Register or deregister the per-catalog import task.
    use crate::tasks::CatalogItemImportTask;
    let task_key = CatalogItemImportTask::task_key(catalog_id);
    if payload.enabled {
        let name = payload.name.unwrap_or_else(|| task_key.clone());
        state
            .tasks
            .register_task(std::sync::Arc::new(CatalogItemImportTask::new(
                catalog_id, &name,
            )))
            .await?;
    } else {
        state.tasks.deregister_task(&task_key).await?;
    }

    Ok(StatusCode::NO_CONTENT)
}

/// MediaSegments stub - returns empty result to prevent 404/CORS errors
/// Fire-and-forget: populate the 24-hour subtitle cache for a movie/episode so
/// that the subsequent playback-info call can read from cache instead of AIO.
fn warm_subtitle_cache(db: &SqlitePool, media: &db::Media) {
    let media = media.clone();
    let db = db.clone();
    tokio::spawn(async move {
        if let Ok(aio) = crate::aio::AioService::from_settings(&db).await {
            let _ = media.get_subtitles(&aio).await;
        }
    });
}

#[get("/mediasegments/{id}")]
pub async fn media_segments(
    _session: auth::AuthSession,
    Path(_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(serde_json::json!({
        "Items": [],
        "TotalRecordCount": 0,
        "StartIndex": 0,
    })))
}
