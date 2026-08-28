use crate::services::{MediaResolveService, StreamService, image::ImageService};
use anyhow::Context;
use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use http::StatusCode;
use itertools::Itertools;
use remux_macros::{delete, get, patch, post, query};
use remux_utils::merge_option;
use serde::Deserialize;
use tracing::{debug, error, info, trace, warn};
use uuid::{Uuid, uuid};

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api,
    common::{IntoVec, TickUnit, ToRunTimeTicks},
    db,
    db::auth,
    errors::LogErr,
    sdks,
};
use axum_anyhow::ApiResult as Result;
use chrono::{Datelike, Utc};
use sqlx::SqlitePool;

use super::{mock_items, stub_json};

pub struct ItemsQueryResult {
    pub items: Vec<api::BaseItemDto>,
    pub total_count: i64,
}

fn apply_permissions(item: &mut api::BaseItemDto, user: &db::User) {
    item.can_delete = Some(
        db::Media::can_delete(user)
            && !matches!(
                item.type_,
                api::MediaType::TvChannel | api::MediaType::Program
            ),
    );
    let allow_download = user
        .policy
        .as_ref()
        .map_or(true, |p| p.enable_content_downloading);
    if !allow_download {
        item.can_download = Some(false);
    }
}

enum ItemsSource {
    Raw(Vec<db::Media>),
    Dtos(Vec<api::BaseItemDto>),
}

pub struct ItemsQueryResultBuilder {
    items: ItemsSource,
    total_count: i64,
    session: auth::AuthSession,
    apply_permissions: bool,
    hide_sources: bool,
    /// Per-client override for CollectionType::Mixed. None = leave as Mixed.
    mixed_collection_type: Option<api::CollectionType>,
}

impl ItemsQueryResultBuilder {
    pub fn with_items(
        session: auth::AuthSession,
        media: Vec<db::Media>,
        total_count: i64,
    ) -> Self {
        Self {
            items: ItemsSource::Raw(media),
            total_count,
            session,
            apply_permissions: false,
            hide_sources: false,
            mixed_collection_type: None,
        }
    }

    pub fn with_dtos(
        session: auth::AuthSession,
        dtos: Vec<api::BaseItemDto>,
        total_count: i64,
    ) -> Self {
        Self {
            items: ItemsSource::Dtos(dtos),
            total_count,
            session,
            apply_permissions: false,
            hide_sources: false,
            mixed_collection_type: None,
        }
    }

    pub fn with_permissions(mut self) -> Self {
        self.apply_permissions = true;
        self
    }

    /// Fill `run_time_ticks` for Playlist rows in the payload before DTO
    /// serialization. Playlists carry no runtime of their own in Remux;
    /// Jellyfin clients expect the summed runtime of the member items.
    /// Also fills Album rows from their child tracks.
    pub async fn preload_playlist_runtimes(mut self, db: &sqlx::SqlitePool) -> Self {
        if let ItemsSource::Raw(ref mut media) = self.items {
            db::Media::preload_playlist_runtimes(db, media).await;
        }
        self
    }

    pub fn with_client_patches(mut self) -> Self {
        let client = self
            .session
            .device
            .jellyfin_client();
        self.hide_sources = client.hide_sources();
        self.mixed_collection_type = client.mixed_collection_type();
        self
    }

    pub fn build(self) -> ItemsQueryResult {
        let mut items: Vec<api::BaseItemDto> = match self.items {
            ItemsSource::Raw(media) => media
                .into_iter()
                .map(|m| {
                    let mut dto = api::db_media_to_item(m, self.hide_sources);
                    if self.apply_permissions {
                        apply_permissions(
                            &mut dto,
                            &self
                                .session
                                .user,
                        );
                    }
                    dto
                })
                .collect(),
            ItemsSource::Dtos(dtos) => {
                if self.apply_permissions {
                    dtos.into_iter()
                        .map(|mut dto| {
                            apply_permissions(
                                &mut dto,
                                &self
                                    .session
                                    .user,
                            );
                            dto
                        })
                        .collect()
                } else {
                    dtos
                }
            }
        };
        for item in &mut items {
            if item.collection_type == Some(api::CollectionType::Mixed) {
                item.collection_type = self
                    .mixed_collection_type
                    .clone();
            }
        }
        ItemsQueryResult {
            items,
            total_count: self.total_count,
        }
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

/// Search results: singles/EPs belong under Tracks, not surfaced as Albums
/// (Deezer `album_kind`). Applies to both live addon results and library hits.
pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
    want_count: bool,
) -> Result<ItemsQueryResultBuilder> {
    if !want_count {
        q.enable_total_record_count = Some(false);
    }
    // Used only by pre-converting paths (search, playlist) that use with_dtos().
    // Raw-media paths delegate hide_sources to with_client_patches() on the builder.
    let client = session
        .device
        .jellyfin_client();
    let hide_sources = client.hide_sources();

    let parent = if let Some(parent_id) = q
        .parent_id
        .clone()
    {
        let resolved = MediaResolveService::resolve_item(parent_id, &state.ctx).await?;
        if let Some(ref m) = resolved {
            if m.id != parent_id {
                q.parent_id = Some(m.id);
            }
        }
        resolved
    } else {
        None
    };

    // Apply the collection's default sort override when the client sends no sort
    // preference or its built-in default sort (treated as "no preference").
    if let Some(ref p) = parent {
        if let Some(ref default_sort) = p.collection_default_sort {
            if !default_sort.is_empty() {
                if client.is_default_sort(&q) {
                    q.sort_by = Some(default_sort.clone());
                    q.sort_order = p
                        .collection_default_sort_order
                        .clone();
                }
            }
        }
    }

    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = server_config
        .stream_groups_show_ungrouped
        .unwrap_or(true);

    let search = q
        .search_term
        .clone();
    let skip = q
        .start_index
        .unwrap_or(0) as u32;

    // "local:" prefix bypasses AIO and falls through to the DB query path below,
    // enabling local title-contains search for any media kind (Genre, Studio, Person, …).
    if let Some(local_term) = search
        .as_deref()
        .and_then(|s| s.strip_prefix("local:"))
    {
        q.search_term = Some(local_term.to_string());
    } else if search.is_some()
        || parent
            .clone()
            .map_or(false, |p| p.kind == db::MediaKind::Collection)
    {
        let types = q.get_requested_item_types();
        let raw_types = q
            .include_item_types
            .as_deref()
            .unwrap_or(&[]);
        let cfg = server_config.clone();

        if let Some(ref s) = search {
            let limit = q
                .limit
                .unwrap_or(250) as usize;

            fn kind_limit(kind: &db::MediaKind, limit: usize) -> usize {
                match kind {
                    db::MediaKind::Track => limit.min(1000),
                    db::MediaKind::Artist | db::MediaKind::Album => limit.min(10),
                    db::MediaKind::Person => limit.min(20),
                    _ => limit,
                }
            }

            fn is_remote_enabled(
                cfg: &api::ServerConfiguration,
                kind: &db::MediaKind,
            ) -> bool {
                match &cfg.search_remote_enabled {
                    None => !matches!(kind, db::MediaKind::TvChannel),
                    Some(list) => list.contains(&kind.to_string()),
                }
            }

            // Requested kinds: explicit from the client, or fall back to the computed
            // defaults (Movie + Series + Episode with exclude_item_types already applied).
            let requested_kinds: Vec<db::MediaKind> = if raw_types.is_empty() {
                types
                    .iter()
                    .filter_map(|t| db::MediaKind::try_from(t.clone()).ok())
                    .collect()
            } else {
                let exclude = q
                    .exclude_item_types
                    .as_deref()
                    .unwrap_or(&[]);
                raw_types
                    .iter()
                    .filter(|t| !exclude.contains(t))
                    .filter_map(|t| db::MediaKind::try_from(t.clone()).ok())
                    .collect()
            };

            let (mut remote_kinds, mut local_kinds): (Vec<_>, Vec<_>) = requested_kinds
                .into_iter()
                .partition(|k| is_remote_enabled(&cfg, k));

            let user_remote_enabled = session
                .user
                .policy
                .as_ref()
                .map(|p| p.enable_remote_search)
                .unwrap_or(true);
            if !user_remote_enabled {
                local_kinds.extend(remote_kinds.drain(..));
            }

            let search_start = std::time::Instant::now();

            // Remote: all in parallel.
            let remote_futs: Vec<_> = remote_kinds
                .iter()
                .map(|k| {
                    state
                        .ctx
                        .addons
                        .search(
                            k,
                            s,
                            kind_limit(k, limit),
                            &state.ctx,
                            Some(
                                session
                                    .user
                                    .id,
                            ),
                        )
                })
                .collect();
            let remote_results = futures::future::join_all(remote_futs).await;

            let mut all_items: Vec<api::BaseItemDto> = vec![];
            let mut debug_counts: Vec<(String, usize)> = vec![];

            for (kind, result) in remote_kinds
                .iter()
                .zip(remote_results)
            {
                match result {
                    Ok(results) => {
                        let items: Vec<_> = results
                            .into_iter()
                            .map(|m| api::db_media_to_item(m, hide_sources))
                            .filter(|item| {
                                q.media_types
                                    .as_ref()
                                    .map_or(true, |mt| mt.contains(&item.media_type))
                            })
                            .collect();
                        debug_counts.push((kind.to_string(), items.len()));
                        all_items.extend(items);
                    }
                    Err(e) => {
                        warn!(error = %e, ?kind, "get_items: remote search failed");
                        debug_counts.push((kind.to_string(), 0));
                    }
                }
            }

            // Local: single DB query for all local kinds combined.
            if !local_kinds.is_empty() {
                let local_types: Vec<api::MediaType> = local_kinds
                    .iter()
                    .map(|k| {
                        k.clone()
                            .into()
                    })
                    .collect();
                let mut local_q = q.clone();
                local_q.search_term = Some(s.clone());
                local_q.include_item_types = Some(local_types);
                local_q.parent_id = None;
                local_q.start_index = None;
                local_q.limit = Some(limit as u32);
                match db::Media::get_by_jellyfin_filter(
                    &state
                        .ctx
                        .db,
                    &local_q,
                    false,
                    Some(&session.user),
                    Some(&server_config),
                    None,
                    None,
                )
                .await
                {
                    Ok(r) => {
                        debug_counts.push((
                            "local".to_string(),
                            r.records
                                .len(),
                        ));
                        all_items.extend(
                            r.records
                                .into_iter()
                                .map(|m| api::db_media_to_item(m, hide_sources)),
                        );
                    }
                    Err(e) => warn!(error = %e, "get_items: local search failed"),
                }
            }

            debug!(
                query = %s,
                counts = %debug_counts.iter().map(|(l, n)| format!("{l}={n}")).collect::<Vec<_>>().join(" "),
                elapsed_ms = search_start.elapsed().as_millis(),
                "search"
            );

            let total_count = all_items.len() as i64;
            let paged_items = all_items
                .into_iter()
                .skip(skip as usize)
                .take(limit)
                .collect();

            return Ok(ItemsQueryResultBuilder::with_dtos(
                session,
                paged_items,
                total_count,
            ));
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
        // playlist browse
        if parent.kind == db::MediaKind::Playlist {
            let relations = db::MediaRelation::get_playlist_items(
                &state
                    .ctx
                    .db,
                &parent.id,
            )
            .await?;
            let total = relations.len() as i64;
            let start = q
                .start_index
                .unwrap_or(0) as usize;
            let remaining = relations
                .len()
                .saturating_sub(start);
            let slice = match q.limit {
                Some(limit) => {
                    &relations[start.min(relations.len())..]
                        [..(limit as usize).min(remaining)]
                }
                None => &relations[start.min(relations.len())..],
            };
            let item_ids: Vec<Uuid> = slice
                .iter()
                .map(|r| r.right_media_id)
                .collect();
            let mut items = Vec::with_capacity(slice.len());
            if !item_ids.is_empty() {
                let mut by_id: std::collections::HashMap<Uuid, db::Media> =
                    db::Media::get_by_ids(
                        &state
                            .ctx
                            .db,
                        &item_ids,
                    )
                    .await?
                    .into_iter()
                    .map(|m| (m.id, m))
                    .collect();
                for rel in slice {
                    if let Some(media) = by_id.remove(&rel.right_media_id) {
                        let mut dto = api::db_media_to_item(media, hide_sources);
                        dto.playlist_item_id = Some(
                            rel.relation_id
                                .to_string(),
                        );
                        items.push(dto);
                    }
                }
            }
            return Ok(ItemsQueryResultBuilder::with_dtos(session, items, total));
        }

        // collection browse
        if parent.kind == db::MediaKind::Collection {
            // Manual group container browse.
            if parent.is_group_container()
                && parent.collection_kind == Some(db::CollectionKind::Manual)
            {
                q.parent_id = Some(parent.id);
                q.include_item_types = Some(vec![api::MediaType::BoxSet]);
                q.include_childless = Some(true);
                q.user_id = Some(
                    session
                        .user
                        .id,
                );
                let result = db::Media::get_by_jellyfin_filter(
                    &state
                        .ctx
                        .db,
                    &q,
                    true,
                    Some(&session.user),
                    Some(&server_config),
                    None,
                    None,
                )
                .await?;
                return Ok(ItemsQueryResultBuilder::with_items(
                    session,
                    result.records,
                    result.total_count as i64,
                ));
            }

            // Smart group container browse.
            if parent.is_group_container()
                && parent.collection_kind == Some(db::CollectionKind::Smart)
            {
                q.promoted = Some(false);
                q.parent_id = None;
                q.include_item_types = Some(vec![api::MediaType::BoxSet]);
                q.include_childless = Some(true);
                q.user_id = Some(
                    session
                        .user
                        .id,
                );
                let smart_filter = parent.parse_smart_filter();
                let result = db::Media::get_by_jellyfin_filter(
                    &state
                        .ctx
                        .db,
                    &q,
                    true,
                    Some(&session.user),
                    Some(&server_config),
                    smart_filter,
                    None,
                )
                .await?;
                return Ok(ItemsQueryResultBuilder::with_items(
                    session,
                    result.records,
                    result.total_count as i64,
                ));
            }

            // Manual collections: use media_relations JOIN via the pre-fetched parent.
            if parent.collection_kind == Some(db::CollectionKind::Manual) {
                q.user_id = Some(
                    session
                        .user
                        .id,
                );
                let result = db::Media::get_by_jellyfin_filter(
                    &state
                        .ctx
                        .db,
                    &q,
                    true,
                    Some(&session.user),
                    Some(&server_config),
                    None,
                    Some(&parent),
                )
                .await?;
                return Ok(ItemsQueryResultBuilder::with_items(
                    session,
                    result.records,
                    result.total_count as i64,
                ));
            }

            // Smart collections: items float freely (no parent_id constraint).
            q.parent_id = None;

            let media_kind_filter = if let Some(kind) = parent
                .collection_media_kind
                .clone()
            {
                match kind {
                    db::CollectionMediaKind::Movie => vec![db::MediaKind::Movie],
                    db::CollectionMediaKind::Series => vec![db::MediaKind::Series],
                    db::CollectionMediaKind::Mixed => {
                        vec![db::MediaKind::Movie, db::MediaKind::Series]
                    }
                    db::CollectionMediaKind::Music => vec![
                        db::MediaKind::Track,
                        db::MediaKind::Album,
                        db::MediaKind::Artist,
                    ],
                    db::CollectionMediaKind::Collection => {
                        // Handled above — this branch is now unreachable for
                        // smart collections with collection_media_kind='collection'.
                        vec![db::MediaKind::Collection]
                    }
                    db::CollectionMediaKind::Playlist => {
                        vec![db::MediaKind::Playlist]
                    }
                }
            } else {
                vec![db::MediaKind::Movie, db::MediaKind::Series]
            };

            q.include_item_types = Some({
                let collection_types: Vec<api::MediaType> = media_kind_filter
                    .iter()
                    .map(|k| {
                        k.clone()
                            .into()
                    })
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
                        vec![]
                    } else {
                        intersection
                    }
                } else {
                    collection_types
                }
            });

            if q.limit
                .is_none()
            {
                q.limit = Some(250);
            }
            q.user_id = Some(
                session
                    .user
                    .id
                    .clone(),
            );

            // Smart collection: extract stored filter rules so they are applied
            // alongside the Jellyfin query (sort, pagination, user-state, etc.).
            let smart_filter =
                if matches!(parent.collection_kind, Some(db::CollectionKind::Smart)) {
                    parent.parse_smart_filter()
                } else {
                    None
                };

            let result = db::Media::get_by_jellyfin_filter(
                &state
                    .ctx
                    .db,
                &q,
                q.enable_total_record_count
                    .unwrap_or(true),
                Some(&session.user),
                Some(&server_config),
                smart_filter,
                Some(&parent),
            )
            .await?;

            return Ok(ItemsQueryResultBuilder::with_items(
                session,
                result.records,
                result.total_count as i64,
            ));
        }

        //  }
    }
    // Map season_id → parent_id if parent_id not already set
    if q.season_id
        .is_some()
        && q.parent_id
            .is_none()
    {
        q.parent_id = q
            .season_id
            .take();
    }

    // Always provide user_id so user-state filters work
    if q.user_id
        .is_none()
    {
        q.user_id = Some(
            session
                .user
                .id,
        );
    }

    let want_total = q
        .enable_total_record_count
        .unwrap_or(true);
    let mut result = db::Media::get_by_jellyfin_filter(
        &state
            .ctx
            .db,
        &q,
        want_total,
        Some(&session.user),
        Some(&server_config),
        None,
        parent.as_ref(),
    )
    .await?;

    // If result is empty, a parent/artist tree may still be mid-persist. Collect all candidate
    // IDs from the query and wait on whichever has (or needs) a persist lock, then retry once.
    if result
        .records
        .is_empty()
    {
        let candidates: Vec<Uuid> = q
            .parent_id
            .iter()
            .chain(
                q.artist_ids
                    .as_deref()
                    .unwrap_or(&[]),
            )
            .chain(
                q.album_artist_ids
                    .as_deref()
                    .unwrap_or(&[]),
            )
            .chain(
                q.contributing_artist_ids
                    .as_deref()
                    .unwrap_or(&[]),
            )
            .copied()
            .collect();

        if MediaResolveService::wait_for_persist(&candidates, &state.ctx).await? {
            result = db::Media::get_by_jellyfin_filter(
                &state
                    .ctx
                    .db,
                &q,
                want_total,
                Some(&session.user),
                Some(&server_config),
                None,
                parent.as_ref(),
            )
            .await?;
        }
    }

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
            let media = item(
                state,
                session.clone(),
                ids[0],
                q.fields
                    .as_deref(),
            )
            .await?;
            if let Some(media) = media {
                return Ok(ItemsQueryResultBuilder::with_dtos(session, vec![media], 1));
            }
        }
    }

    Ok(ItemsQueryResultBuilder::with_items(
        session,
        result.records,
        result.total_count as i64,
    ))
}

#[get("/items/latest")]
pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(mut q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // Jellyfin ignores the MediaTypes query parameter for this request.
    // Without this, supplying a value (e.g. Video) would exclude Series collections.
    q.media_types = None;
    let client = session
        .device
        .jellyfin_client();
    if let Some(parent_id) = q
        .parent_id
        .clone()
    {
        let resolved = MediaResolveService::resolve_item(parent_id, &state.ctx).await?;
        if let Some(ref parent) = resolved {
            if parent.id != parent_id {
                q.parent_id = Some(parent.id);
            }
            if parent.collection_latest_auto_unplayed == Some(true) {
                let mut filters = q
                    .filters
                    .clone()
                    .unwrap_or_default();
                if !filters.contains(&api::ItemFilter::IsUnplayed) {
                    filters.push(api::ItemFilter::IsUnplayed);
                }
                q.filters = Some(filters);
                q.user_id = Some(
                    session
                        .user
                        .id
                        .clone(),
                );
            }
            if parent.collection_latest_sort_digital == Some(true) {
                q.sort_by = Some(vec![api::ItemSortBy::DigitalReleaseDate]);
                q.sort_order = Some(vec![api::SortOrder::Descending]);
            } else if let Some(ref default_sort) = parent.collection_default_sort {
                if !default_sort.is_empty() {
                    if client.is_default_sort(&q) {
                        q.sort_by = Some(default_sort.clone());
                        q.sort_order = parent
                            .collection_default_sort_order
                            .clone();
                    }
                }
            }
        }
    }
    if q.sort_by
        .is_none()
    {
        q.sort_by = Some(vec![api::ItemSortBy::DateCreated]);
        q.sort_order = Some(vec![api::SortOrder::Descending]);
    }
    let items = get_items(state.clone(), session.clone(), q, false)
        .await?
        .with_permissions()
        .with_client_patches()
        .build();
    Ok(Json::<Vec<api::BaseItemDto>>(items.items))
}

#[get("/items", "/items/")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state.clone(), session.clone(), q.clone(), true)
        .await?
        .preload_playlist_runtimes(
            &state
                .ctx
                .db,
        )
        .await
        .with_permissions()
        .with_client_patches()
        .build();

    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

/// Return the virtual root folder
#[get("/items/root")]
pub async fn items_root(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDto {
        id: uuid!("f47ac10b-58cc-4372-a567-0e02b2c3d479"),
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
    let ancestors = db::Media::get_ancestors(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;
    Ok(Json(
        ancestors
            .into_iter()
            .map(|m| api::db_media_to_item(m, false))
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
    db::Media::delete(
        &state
            .ctx
            .db,
        &id,
    )
    .await?;
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::LibraryChanged);
    Ok(StatusCode::NO_CONTENT)
}

#[post("/items/{id}/refresh")]
pub async fn refresh_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::RefreshItemQuery>,
) -> Result<StatusCode> {
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("Item not found")?;

    // If the requested item is a Source (stream), navigate to its parent.
    if media.kind == db::MediaKind::Stream {
        let parent_id = media
            .parent_id
            .context_not_found("Source has no parent item")?;
        media = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &parent_id,
        )
        .await?
        .context_not_found("Parent item not found")?;
    }

    // new files
    if q.metadata_refresh_mode == api::MetadataRefreshMode::Default {
        // Invalidate the stream cache. The next request that hits this item
        // will re-fetch streams with the requesting user's addon scope.
        // Do NOT call refresh_streams here — this is an admin endpoint with no user context.
        sqlx::query("UPDATE media SET streams_refreshed_at = NULL WHERE id = ?")
            .bind(media.id)
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await
            .ok();

        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
            warm_providers_cache(&state.ctx, &media);
        }
    } else if q.metadata_refresh_mode == api::MetadataRefreshMode::FullRefresh {
        let force_refresh = q.replace_all_metadata;
        state
            .ctx
            .addons
            .process_meta_batch(vec![media], &state.ctx, force_refresh, None)
            .await?;
    }

    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::LibraryChanged);
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
    let genres = db::Media::get_genres(
        &state
            .ctx
            .db,
        &kinds,
    )
    .await?;
    let tag_rows = sqlx::query("SELECT DISTINCT tag FROM media_tags ORDER BY tag")
        .fetch_all(
            &state
                .ctx
                .db,
        )
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
    let tags: Vec<String> = match q
        .search_term
        .as_deref()
    {
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
            .fetch_all(
                &state
                    .ctx
                    .db,
            )
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
    let values: Vec<String> = match q
        .search_term
        .as_deref()
    {
        Some(s) if !s.is_empty() => {
            let pattern = format!("%{}%", s.to_lowercase());
            sqlx::query(
                "SELECT DISTINCT certification FROM media \
                 WHERE certification IS NOT NULL AND lower(certification) LIKE ? \
                 ORDER BY certification LIMIT 25",
            )
            .bind(&pattern)
            .fetch_all(
                &state
                    .ctx
                    .db,
            )
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
        .fetch_all(
            &state
                .ctx
                .db,
        )
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

/// List distinct production countries from media_relations, optionally filtered by search_term
#[get("/items/countries")]
pub async fn items_countries(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let values: Vec<String> = match q
        .search_term
        .as_deref()
    {
        Some(s) if !s.is_empty() => {
            let pattern = format!("%{}%", s.to_lowercase());
            sqlx::query(
                "SELECT DISTINCT title FROM media \
                 WHERE kind = 'country' AND lower(title) LIKE ? \
                 ORDER BY title LIMIT 25",
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
            "SELECT DISTINCT title FROM media WHERE kind = 'country' ORDER BY title LIMIT 50",
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

/// List distinct original_language codes from media, optionally filtered by search_term
#[get("/items/languages")]
pub async fn items_languages(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let values: Vec<String> = match q
        .search_term
        .as_deref()
    {
        Some(s) if !s.is_empty() => {
            let pattern = format!("%{}%", s.to_lowercase());
            sqlx::query(
                "SELECT DISTINCT original_language FROM media \
                 WHERE original_language IS NOT NULL AND lower(original_language) LIKE ? \
                 ORDER BY original_language LIMIT 25",
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
            "SELECT DISTINCT original_language FROM media \
             WHERE original_language IS NOT NULL \
             ORDER BY original_language LIMIT 50",
        )
        .fetch_all(
            &state
                .ctx
                .db,
        )
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

/// Trigger a full library refresh (re-imports all enabled catalogs)
#[post("/library/refresh")]
pub async fn library_refresh(
    State(state): State<AppState>,
    _session: auth::AdminSession,
) -> Result<StatusCode> {
    let _ = state
        .tasks
        .run_task("RefreshLibrary")
        .await;
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
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::ThemeMediaResult {
        owner_id: id.to_string(),
        ..Default::default()
    }))
}

#[get("/items/{id}/themesongs")]
pub async fn items_theme_songs(
    _state: State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::ThemeMediaResult {
        owner_id: id.to_string(),
        ..Default::default()
    }))
}

#[get("/items/{id}/intros")]
pub async fn items_intros(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    crate::api::intro::get_intros_inner(state, session, id).await
}

#[query]
#[derive(Debug, Default)]
pub struct RemoteImagesQuery {
    #[serde(rename = "type", alias = "Type")]
    pub kind: Option<String>,
    pub include_all_languages: Option<bool>,
    pub start_index: Option<i64>,
    pub limit: Option<i64>,
    pub provider: Option<String>,
}

/// Resolve high-resolution images from TMDB for any media kind.
/// The Jellyfin web client hits this endpoint to upgrade
/// AIO's hardcoded ~500w thumbnails to original-size posters / backdrops /
/// stills. Without this, episodes show pixelated banners.
#[get("/items/{id}/remoteimages")]
pub async fn items_remote_images(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<RemoteImagesQuery>,
) -> Result<impl IntoResponse> {
    let media = match db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    {
        Some(m) => m,
        None => {
            return Ok(Json(api::RemoteImageResult {
                images: Some(vec![]),
                total_record_count: 0,
                providers: Some(vec!["TheMovieDb".to_string()]),
            }));
        }
    };

    let provider = q
        .provider
        .as_deref();
    let mut images = Vec::new();
    let mut queried_providers = Vec::new();

    if provider.is_none() || provider == Some("TheMovieDb") {
        queried_providers.push("TheMovieDb".to_string());
        match state
            .ctx
            .addons
            .fetch_images(&media, &state.ctx)
            .await
        {
            Ok(v) => images.extend(v),
            Err(e) => warn!(id = %id, error = %e, "tmdb remote images lookup failed"),
        }
    }

    // Optional client-side type filter (Backdrop / Primary / etc.).
    let images: Vec<api::RemoteImageInfo> = if let Some(want) = q
        .kind
        .as_deref()
    {
        let want = want.to_string();
        images
            .into_iter()
            .filter(|img| {
                img.type_
                    .as_deref()
                    == Some(&want)
            })
            .collect()
    } else {
        images
    };

    let start = q
        .start_index
        .unwrap_or(0)
        .max(0) as usize;
    let total = images.len() as i64;
    let limited: Vec<api::RemoteImageInfo> = images
        .into_iter()
        .skip(start)
        .take(
            q.limit
                .map(|n| n.max(0) as usize)
                .unwrap_or(usize::MAX),
        )
        .collect();

    Ok(Json(api::RemoteImageResult {
        images: Some(limited),
        total_record_count: total,
        providers: Some(queried_providers),
    }))
}

#[get("/items/{id}/remoteimages/providers")]
pub async fn items_remote_images_providers(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    #[derive(serde::Serialize)]
    struct ImageProviderInfo {
        #[serde(rename = "Name")]
        name: &'static str,
        #[serde(rename = "SupportedImages")]
        supported_images: Vec<&'static str>,
    }
    Ok(Json(vec![ImageProviderInfo {
        name: "TheMovieDb",
        supported_images: vec!["Primary", "Backdrop", "Thumb", "Logo"],
    }]))
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
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Movie
        ),
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Series
        ),
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Episode
        ),
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Track
        ),
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Album
        ),
        db::Media::count_by_kind(
            &state
                .ctx
                .db,
            &db::MediaKind::Artist
        ),
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
    item_for_user(state, session, id, fields, None).await
}

async fn item_for_user(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
    _fields: Option<&[api::ItemFields]>,
    target_user_id: Option<Uuid>,
) -> Result<Option<api::BaseItemDto>> {
    let want_streams = true;
    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = server_config
        .stream_groups_show_ungrouped
        .unwrap_or(true);
    let encoding_cfg = db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    let transcoding_enabled = encoding_cfg
        .enable_video_transcoding
        .unwrap_or(true);
    // Clients that switch versions (Android TV) refetch the item by MediaSource id
    // and then play MediaSources[0], so the requested group must end up first and
    // keep its own UUID instead of the item id stamped by `db_media_to_item`.
    let mut requested_group: Option<Uuid> = None;
    let resolved_id = match MediaResolveService::resolve_item(id, &state.ctx).await? {
        Some(m) if m.kind == db::MediaKind::StreamGroup => {
            requested_group = Some(m.id);
            // Stream groups have no parent_id on the row (they're global). Look up the
            // group→item mapping written by the items pipeline and PlaybackInfo.
            StreamService::get_group_item(
                &state
                    .ctx
                    .store,
                target_user_id.unwrap_or(
                    session
                        .user
                        .id,
                ),
                m.id,
            )
            .context_not_found("stream group not yet associated with an item")?
        }
        Some(m) => m.id,
        None => return Ok(None),
    };
    let mut media = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            id: Some(vec![resolved_id]),
            include_user_state: true,
            include_child_count: true,
            user_id: Some(
                target_user_id.unwrap_or(
                    session
                        .user
                        .id,
                ),
            ),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .context_not_found("item not found")?;

    db::Media::preload_playlist_runtimes(
        &state
            .ctx
            .db,
        std::slice::from_mut(&mut media),
    )
    .await;

    let needs_streams = want_streams
        && matches!(
            media.kind,
            db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
        );

    if needs_streams {
        if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
            warm_providers_cache(&state.ctx, &media);
        }
        state
            .ctx
            .addons
            .refresh_streams(
                &mut media,
                &state.ctx,
                Some(
                    session
                        .user
                        .id,
                ),
            )
            .await
            .log_err("failed to refresh sources");
    }

    let user_stream_filter = session
        .user
        .policy
        .as_ref()
        .and_then(|p| {
            p.stream_filter
                .as_ref()
        })
        .filter(|sf| {
            !sf.rules
                .is_empty()
        })
        .cloned();
    if want_streams && media.kind == db::MediaKind::Stream {
        media.sources = Some(vec![media.clone()]);
    } else if want_streams
        && matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
    {
        let raw = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?;
        let grouped = db::StreamGroup::filter_sources(
            &state
                .ctx
                .db,
            raw,
            show_ungrouped,
        )
        .await;
        let filtered = if let Some(ref sf) = user_stream_filter {
            db::apply_stream_filter(sf, grouped)
        } else {
            grouped
        };
        for source in &filtered {
            if let Some(gid) = source.group_id {
                StreamService::save_group_item(
                    &state
                        .ctx
                        .store,
                    session
                        .user
                        .id,
                    gid,
                    media.id,
                );
            }
        }
        // The other versions stay in the list so the version picker is complete.
        let mut filtered = filtered;
        if let Some(gid) = requested_group {
            match filtered
                .iter()
                .position(|s| s.group_id == Some(gid))
            {
                Some(pos) => {
                    let source = filtered.remove(pos);
                    filtered.insert(0, source);
                }
                None => {
                    warn!(%gid, item = %media.id, "requested stream group has no matching source");
                    requested_group = None;
                }
            }
        }
        media.sources = Some(filtered);
        media
            .user_state(
                &state
                    .ctx
                    .db,
                &session.user,
            )
            .await?;
    } else if want_streams && media.kind == db::MediaKind::Track {
        let raw = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?;
        let grouped = db::StreamGroup::filter_sources(
            &state
                .ctx
                .db,
            raw,
            show_ungrouped,
        )
        .await;
        let filtered = if let Some(ref sf) = user_stream_filter {
            db::apply_stream_filter(sf, grouped)
        } else {
            grouped
        };
        for source in &filtered {
            if let Some(gid) = source.group_id {
                StreamService::save_group_item(
                    &state
                        .ctx
                        .store,
                    session
                        .user
                        .id,
                    gid,
                    media.id,
                );
            }
        }
        media.sources = Some(filtered);
        media
            .user_state(
                &state
                    .ctx
                    .db,
                &session.user,
            )
            .await?;
    }
    // info!("Seasons length: {:?}", media.seasons(&state.ctx.db).await?.len());
    media
        .load_relations(
            &state
                .ctx
                .db,
        )
        .await?;
    let mut base_item = api::db_media_to_item(media.clone(), false);

    if !transcoding_enabled {
        if let Some(sources) = base_item
            .media_sources
            .as_mut()
        {
            for source in sources.iter_mut() {
                source.supports_transcoding = false;
            }
        }
    }

    // `db_media_to_item` stamps MediaSources[0].Id with the item id (clients rely on
    // that for auto-play). Undo it for a group request: the item id would resolve
    // back to the highest-priority group on PlaybackInfo (issue #220).
    let hoisted_group = requested_group.filter(|gid| {
        media
            .sources
            .as_ref()
            .and_then(|s| s.first())
            .and_then(|s| s.group_id)
            == Some(*gid)
    });
    if let Some(gid) = hoisted_group {
        if let Some(source) = base_item
            .media_sources
            .as_mut()
            .and_then(|s| s.first_mut())
        {
            source.id = gid;
            source.e_tag = gid;
        }
    }

    // When streams were actually fetched but none found, replace the
    // listing-style stubs with a single "No streams found" stub.
    if needs_streams
        && matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
        && media
            .sources
            .as_deref()
            .map_or(false, |s| s.is_empty())
    {
        let media_streams = media
            .probe_data
            .as_ref()
            .map(|p| {
                p.media_streams
                    .clone()
            })
            .unwrap_or_default();
        base_item.media_sources = Some(vec![api::MediaSourceInfo {
            id: media.id,
            e_tag: media.id,
            name: Some("No streams found".to_string()),
            protocol: api::MediaProtocol::File,
            path: Some(format!("/remux/{}", media.id)),
            media_streams,
            ..Default::default()
        }]);
    }

    // For tracks, wrap the Source row(s) as HLS-transcoded MediaSources.
    // CDN URLs are IP-locked to the server; the client must go through the HLS pipeline.
    if media.kind == db::MediaKind::Track {
        let transcoding_url = format!(
            "/videos/{}/master.m3u8?MediaSourceId={}&VideoCodec=copy&AudioCodec=aac&ApiKey={}",
            media.id,
            media.id,
            session
                .device
                .access_token
                .expose()
        );
        let sources = media
            .sources
            .as_deref()
            .unwrap_or(&[]);
        let mut media_streams: Vec<api::MediaStream> = sources
            .first()
            .and_then(|s| {
                s.probe_data
                    .as_ref()
            })
            .map(|p| {
                let mut streams = p
                    .media_streams
                    .clone();
                for s in &mut streams {
                    if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                        s.is_text_subtitle_stream = s.is_text_subtitle_stream();
                    }
                }
                streams
            })
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
            name: Some(
                media
                    .title
                    .clone(),
            ),
            protocol: api::MediaProtocol::Http,
            is_remote: true,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: transcoding_enabled,
            transcoding_url: Some(transcoding_url),
            transcoding_sub_protocol: "hls".to_string(),
            transcoding_container: Some("ts".to_string()),
            run_time_ticks: sources
                .first()
                .and_then(|s| {
                    s.probe_data
                        .as_ref()
                })
                .and_then(|p| p.run_time_ticks)
                .or_else(|| {
                    media
                        .runtime
                        .and_then(|r| r.to_ticks(TickUnit::Seconds))
                }),
            media_streams,
            ..Default::default()
        };
        api::inject_lyric_stream(&mut source);
        base_item.media_sources = Some(vec![source]);
    }

    if media.kind == db::MediaKind::Episode {
        if let Some(sid) = media.grandparent_id {
            if let Ok(Some(s)) = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &sid,
            )
            .await
            {
                base_item.series_name = Some(s.title);
                base_item.series_id = Some(s.id);
            }
        }
        if let Some(pid) = media.parent_id {
            if let Ok(Some(s)) = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &pid,
            )
            .await
            {
                base_item.season_name = Some(s.title);
                base_item.season_id = Some(s.id);
            }
        }
    } else if media.kind == db::MediaKind::Season {
        if let Some(pid) = media.parent_id {
            if let Ok(Some(s)) = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &pid,
            )
            .await
            {
                base_item.series_name = Some(s.title);
                base_item.series_id = Some(s.id);
            }
        }
    }
    if want_streams
        && media
            .sources
            .as_ref()
            .is_none_or(|s| s.is_empty())
        && !matches!(
            media.kind,
            db::MediaKind::TvChannel | db::MediaKind::TvProgram
        )
    {
        base_item.location_type = api::LocationType::Virtual;
        base_item.path = None;
        base_item.can_download = Some(false);
    }

    if want_streams {
        // Language defaults must apply even when the user has never saved a
        // configuration (configuration is NULL for brand-new users) — the server's
        // global metadata language is the fallback for subtitle selection.
        let cfg = session
            .user
            .configuration
            .as_ref()
            .map(|c| {
                c.0.clone()
            })
            .unwrap_or_default();
        if let Some(ref mut sources) = base_item.media_sources {
            // Default audio/subtitle stream indexes are per-request API values
            // (never persisted) — derive them here for the detail page.
            for source in sources.iter_mut() {
                source.resolve_default_streams(
                    &cfg,
                    server_config
                        .preferred_metadata_language
                        .as_deref(),
                    media
                        .original_language
                        .as_deref(),
                    None,
                    None,
                    None,
                    None,
                );
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
    auth::TargetUser(target): auth::TargetUser,
    Path(id): Path<Uuid>,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    return Ok(Json(
        item_for_user(
            state,
            session,
            id,
            q.fields
                .as_deref(),
            Some(target.id),
        )
        .await?
        .context_not_found("item not found")?,
    )
    .into_response());
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
        .with_permissions()
        .with_client_patches()
        .build();
    Ok(Json(api::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
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

    let genres = db::Media::get_genres(
        &state
            .ctx
            .db,
        &kinds,
    )
    .await?;
    let years = db::Media::get_distinct_years(
        &state
            .ctx
            .db,
        &kinds,
    )
    .await?;

    Ok(Json(api::QueryFiltersLegacy {
        genres: Some(
            genres
                .into_iter()
                .map(|g| g.title)
                .collect(),
        ),
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
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
            promoted: Some(true),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .map(|x| api::db_media_to_item(x, false))
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
        &state
            .ctx
            .db,
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
        .and_then(api::db_media_kind_to_collection_type);
    api::VirtualFolderInfo {
        name: Some(
            m.title
                .clone(),
        ),
        item_id: Some(m.id.to_string()),
        collection_type,
        collection_kind: m
            .collection_kind
            .as_ref()
            .map(|k| k.to_string()),
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
    sort_order: Option<i64>,
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

    require_valid_group_kind(collection_media_kind.as_ref(), Some(&collection_kind))?;

    let promoted = payload
        .promoted
        .unwrap_or(false);

    let mut media = db::Media {
        title: payload.name,
        kind: db::MediaKind::Collection,
        collection_kind: Some(collection_kind.clone()),
        collection_media_kind,
        promoted,
        sort_order: payload.sort_order,
        ..Default::default()
    };

    media
        .save(
            &state
                .ctx
                .db,
        )
        .await?;

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
    sort_order: Option<i64>,
}

#[post("/library/virtualfolders/LibraryOptions")]
pub async fn update_virtual_folder(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Json(payload): Json<UpdateVirtualFolderRequest>,
) -> Result<StatusCode> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &payload.id,
    )
    .await?
    .context_not_found("Collection not found")?;

    if media.kind != db::MediaKind::Collection {
        return Err(anyhow::anyhow!("not a collection"))
            .context_bad_request("Item is not a collection");
    }

    let collection_media_kind = payload
        .collection_type
        .as_deref()
        .and_then(|s| parse_collection_type(s));

    let collection_kind = payload
        .collection_kind
        .as_deref()
        .and_then(|s| db::CollectionKind::try_from(s).ok());

    require_valid_group_kind(collection_media_kind.as_ref(), collection_kind.as_ref())?;

    let promoted = payload
        .promoted
        .unwrap_or(false);
    let updated_at = Utc::now().naive_utc();

    sqlx::query(
        "UPDATE media SET title = $1, promoted = $2, collection_media_kind = $3, collection_kind = $4, collection_max_items = $5, updated_at = $6, sort_order = $8 WHERE id = $7",
    )
    .bind(&payload.name)
    .bind(promoted)
    .bind(collection_media_kind.as_ref().map(|k| k.to_string()))
    .bind(collection_kind.as_ref().map(|k| k.to_string()))
    .bind(payload.collection_max_items)
    .bind(updated_at)
    .bind(payload.id)
    .bind(payload.sort_order)
    .execute(&state.ctx.db)
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

#[query]
#[derive(Debug)]
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
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Collection]),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .find(|m| m.title == q.name);

    let media = result.context_not_found("Collection not found")?;

    db::Media::delete(
        &state
            .ctx
            .db,
        &media.id,
    )
    .await?;

    Ok(StatusCode::NO_CONTENT)
}

fn require_valid_group_kind(
    media_kind: Option<&db::CollectionMediaKind>,
    collection_kind: Option<&db::CollectionKind>,
) -> Result<()> {
    if media_kind == Some(&db::CollectionMediaKind::Collection)
        && !matches!(
            collection_kind,
            Some(&db::CollectionKind::Manual) | Some(&db::CollectionKind::Smart)
        )
    {
        return Err(anyhow::anyhow!(
            "collection_kind must be Manual or Smart when collection_type is collections"
        ))
        .context_bad_request(
            "Group containers must use Manual or Smart collection kind",
        );
    }
    Ok(())
}

fn parse_collection_type(s: &str) -> Option<db::CollectionMediaKind> {
    match s {
        "movies" => Some(db::CollectionMediaKind::Movie),
        "tvshows" => Some(db::CollectionMediaKind::Series),
        "mixed" => Some(db::CollectionMediaKind::Mixed),
        "music" => Some(db::CollectionMediaKind::Music),
        "collections" => Some(db::CollectionMediaKind::Collection),
        "playlists" => Some(db::CollectionMediaKind::Playlist),
        _ => None,
    }
}

#[get("/genres")]
pub async fn genres(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let parent = if let Some(pid) = q.parent_id {
        db::Media::get_by_id(
            &state
                .ctx
                .db,
            &pid,
        )
        .await?
    } else {
        None
    };

    // Smart collections have no parent_id-linked children; scope genres by content kind.
    let is_music = parent
        .as_ref()
        .map_or(false, |p| {
            p.collection_media_kind == Some(db::CollectionMediaKind::Music)
        });
    let genre_related_kinds = parent
        .as_ref()
        .and_then(|p| {
            if p.kind != db::MediaKind::Collection
                || p.collection_kind == Some(db::CollectionKind::Manual)
            {
                return None;
            }
            Some(match &p.collection_media_kind {
                Some(db::CollectionMediaKind::Music) => vec![
                    db::MediaKind::Track,
                    db::MediaKind::Album,
                    db::MediaKind::Artist,
                ],
                Some(db::CollectionMediaKind::Movie) => vec![db::MediaKind::Movie],
                Some(db::CollectionMediaKind::Series) => {
                    vec![db::MediaKind::Series, db::MediaKind::Episode]
                }
                Some(db::CollectionMediaKind::Mixed) => vec![
                    db::MediaKind::Movie,
                    db::MediaKind::Series,
                    db::MediaKind::Episode,
                ],
                _ => vec![
                    db::MediaKind::Movie,
                    db::MediaKind::Series,
                    db::MediaKind::Episode,
                ],
            })
        });

    let kind_filter = if is_music {
        vec![db::MediaKind::MusicGenre]
    } else {
        vec![db::MediaKind::Genre, db::MediaKind::MusicGenre]
    };

    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(kind_filter),
            limit: q.limit,
            offset: q.start_index,
            total_count: true,
            genre_related_kinds,
            sort_by: q
                .sort_by
                .unwrap_or_default(),
            sort_order: q
                .sort_order
                .unwrap_or_default(),
            title_contains: q.search_term,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(api::BaseItemDtoQueryResult {
        items: result
            .records
            .into_iter()
            .map(|m| api::db_media_to_item(m, false))
            .collect(),
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
        ..Default::default()
    }))
}

#[get("/musicgenres")]
pub async fn music_genres(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let genre_related_kinds = if q
        .parent_id
        .is_some()
    {
        Some(vec![
            db::MediaKind::Track,
            db::MediaKind::Album,
            db::MediaKind::Artist,
        ])
    } else {
        None
    };

    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::MusicGenre]),
            limit: q.limit,
            offset: q.start_index,
            total_count: true,
            genre_related_kinds,
            sort_by: q
                .sort_by
                .unwrap_or_default(),
            sort_order: q
                .sort_order
                .unwrap_or_default(),
            title_contains: q.search_term,
            ..Default::default()
        },
    )
    .await?;

    Ok(Json(api::BaseItemDtoQueryResult {
        items: result
            .records
            .into_iter()
            .map(|m| api::db_media_to_item(m, false))
            .collect(),
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
        ..Default::default()
    }))
}

/// `/MusicGenres/{name}` — returns a single music genre item by display name.
#[get("/musicgenres/{name}")]
pub async fn get_music_genre_by_name(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    use crate::OptionExt;
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM media WHERE kind = 'genre' AND LOWER(title) = LOWER(?) LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(
        &state
            .ctx
            .db,
    )
    .await?
    .context_not_found("Genre not found")?;
    let genre = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("Genre not found")?;
    Ok(Json(api::db_media_to_item(genre, false)))
}

#[get("/items/{id}/metadataeditor")]
pub async fn items_metadata_editor(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let item = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("Item not found")?;

    let config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await?;
    let parental_rating_options =
        crate::localization::ratings::parental_ratings_for_country(
            config
                .metadata_country_code
                .as_deref(),
        );

    let countries: Vec<api::CountryInfo> = rust_iso3166::ALL
        .iter()
        .map(|c| api::CountryInfo {
            name: c
                .name
                .to_string(),
            display_name: c
                .name
                .to_string(),
            two_letter_iso_region_name: c
                .alpha2
                .to_string(),
            three_letter_iso_region_name: c
                .alpha3
                .to_string(),
        })
        .collect();

    let mut cultures: Vec<api::CultureDto> = isolang::languages()
        .filter_map(|lang| {
            let two = lang.to_639_1()?;
            Some(api::CultureDto {
                name: lang
                    .to_name()
                    .to_string(),
                display_name: lang
                    .to_name()
                    .to_string(),
                two_letter_iso_language_name: two.to_string(),
                three_letter_iso_language_name: lang
                    .to_639_3()
                    .to_string(),
                three_letter_iso_language_names: vec![
                    lang.to_639_3()
                        .to_string(),
                ],
            })
        })
        .collect();
    cultures.sort_by(|a, b| {
        a.display_name
            .cmp(&b.display_name)
    });

    let external_id_infos: Vec<api::ExternalIdInfo> = vec![
        ("IMDb", "Imdb", None),
        ("TheMovieDb", "Tmdb", Some("Movie")),
        ("TheMovieDb", "TmdbCollection", Some("BoxSet")),
        ("TheTVDB", "TvdbCollection", Some("BoxSet")),
        ("TheTVDB Numerical", "Tvdb", Some("Movie")),
        ("TheTVDB Slug", "TvdbSlug", Some("Movie")),
    ]
    .into_iter()
    .map(|(name, key, type_)| api::ExternalIdInfo {
        name: name.to_string(),
        key: key.to_string(),
        type_: type_.map(str::to_string),
        url_format_string: None,
    })
    .collect();

    let content_type_options: Vec<String> = vec![
        db::MediaKind::Movie,
        db::MediaKind::Series,
        db::MediaKind::Season,
        db::MediaKind::Episode,
        db::MediaKind::Artist,
        db::MediaKind::Album,
        db::MediaKind::Track,
        db::MediaKind::Playlist,
    ]
    .into_iter()
    .map(|k| k.to_string())
    .collect();

    Ok(Json(api::MetadataEditorInfo {
        parental_rating_options,
        countries,
        cultures,
        external_id_infos,
        content_type: Some(
            item.kind
                .to_string(),
        ),
        content_type_options,
    }))
}

#[query]
#[derive(Debug, Default)]
struct GetSimilarItemsQuery {
    pub user_id: Option<Uuid>,
    pub limit: Option<u32>,
    pub start_index: Option<u32>,
    pub fields: Option<Vec<api::ItemFields>>,
}

#[get("/items/{id}/similar")]
pub async fn items_similar(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<GetSimilarItemsQuery>,
) -> Result<impl IntoResponse> {
    let limit = q
        .limit
        .unwrap_or(12)
        .min(50) as u32;
    let offset = q
        .start_index
        .unwrap_or(0);

    let (scored_ids, total) = db::Media::get_similar_by_genres(
        &state
            .ctx
            .db,
        &id,
        limit,
        offset,
    )
    .await?;

    if scored_ids.is_empty() {
        return Ok(Json(api::BaseItemDtoQueryResult {
            ..Default::default()
        }));
    }

    // Fetch full items in score order.
    let ids: Vec<Uuid> = scored_ids
        .iter()
        .map(|(id, _)| *id)
        .collect();
    let filter = db::MediaFilter {
        id: Some(ids),
        user_id: q
            .user_id
            .or(Some(
                session
                    .user
                    .id,
            )),
        include_user_state: true,
        ..Default::default()
    };
    let result = db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &filter,
    )
    .await?;

    // Reorder results to match score order.
    let score_map: std::collections::HashMap<Uuid, i64> = scored_ids
        .into_iter()
        .collect();
    let mut items: Vec<api::BaseItemDto> = result
        .records
        .into_iter()
        .map(|m| api::db_media_to_item(m, false))
        .collect();
    items.sort_by_key(|item| {
        let id = item.id;
        std::cmp::Reverse(
            score_map
                .get(&id)
                .copied()
                .unwrap_or(0),
        )
    });

    Ok(Json(api::BaseItemDtoQueryResult {
        items,
        total_record_count: total,
        start_index: offset,
        ..Default::default()
    }))
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

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemPerson {
    id: Option<Uuid>,
    name: String,
    #[serde(rename = "Type")]
    type_: Option<String>,
    role: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct UpdateItemRequest {
    name: Option<String>,
    overview: Option<String>,
    premiere_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(
        default,
        deserialize_with = "remux_sdks::deserialize_option_i64_from_string"
    )]
    production_year: Option<i64>,
    official_rating: Option<String>,
    #[serde(
        default,
        deserialize_with = "remux_sdks::deserialize_option_number_from_string"
    )]
    community_rating: Option<f64>,
    #[serde(
        default,
        deserialize_with = "remux_sdks::deserialize_option_number_from_string"
    )]
    critic_rating: Option<f64>,
    tags: Option<Vec<String>>,
    genres: Option<Vec<String>>,
    people: Option<Vec<UpdateItemPerson>>,
    locked_fields: Option<Vec<db::MetadataField>>,
    lock_data: Option<bool>,
}

#[post("/items/{id}")]
pub async fn update_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<UpdateItemRequest>,
) -> Result<StatusCode> {
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("Item not found")?;

    if let Some(name) = payload.name {
        media.title = name;
    }
    merge_option(&mut media.description, &payload.overview, true);
    if let Some(premiere_date) = payload.premiere_date {
        media.released_at = Some(premiere_date.naive_utc());
    } else if let Some(year) = payload.production_year {
        if let Some(dt) = chrono::NaiveDate::from_ymd_opt(year as i32, 1, 1)
            .and_then(|d| d.and_hms_opt(0, 0, 0))
        {
            media.released_at = Some(dt);
        }
    }
    merge_option(&mut media.certification, &payload.official_rating, true);
    merge_option(&mut media.rating_audience, &payload.community_rating, true);
    merge_option(&mut media.rating_critic, &payload.critic_rating, true);
    if let Some(locked_fields) = payload.locked_fields {
        media.locked_fields = locked_fields;
    }
    if let Some(lock_data) = payload.lock_data {
        media.is_locked = lock_data;
    }
    media
        .save(
            &state
                .ctx
                .db,
        )
        .await
        .context_bad_request("Failed to save item")?;

    if let Some(tags) = &payload.tags {
        set_tags(
            &state
                .ctx
                .db,
            id,
            tags,
        )
        .await
        .context_bad_request("Failed to update tags")?;
    }

    if let Some(genres) = &payload.genres {
        db::MediaRelation::delete_by_right_kinds(
            &state
                .ctx
                .db,
            id,
            &[db::MediaKind::Genre, db::MediaKind::MusicGenre],
        )
        .await?;
        if !genres.is_empty() {
            let pairs =
                db::build_genre_relations_from_names(id, genres, db::MediaKind::Genre);
            let medias: Vec<_> = pairs
                .iter()
                .map(|(_, m)| m.clone())
                .collect();
            let rels: Vec<_> = pairs
                .into_iter()
                .map(|(r, _)| r)
                .collect();
            db::Media::upsert(
                &state
                    .ctx
                    .db,
                &medias,
            )
            .await
            .inspect_err(|e| warn!(error = %e, "failed to upsert genre media"))
            .ok();
            db::MediaRelation::upsert(
                &state
                    .ctx
                    .db,
                &rels,
            )
            .await
            .inspect_err(|e| warn!(error = %e, "failed to upsert genre relations"))
            .ok();
        }
    }

    if let Some(people) = &payload.people {
        db::MediaRelation::delete_by_right_kinds(
            &state
                .ctx
                .db,
            id,
            &[db::MediaKind::Person],
        )
        .await?;
        if !people.is_empty() {
            // Resolve person IDs: prefer the Id supplied by the client (which the
            // client echoes back from our own response), then fall back to a name
            // lookup so we don't create a duplicate record and lose images.
            let names_needing_lookup: Vec<&str> = people
                .iter()
                .filter(|p| {
                    p.id.is_none()
                })
                .map(|p| {
                    p.name
                        .as_str()
                })
                .collect();

            let name_to_id: std::collections::HashMap<String, Uuid> =
                if names_needing_lookup.is_empty() {
                    Default::default()
                } else {
                    let mut map = std::collections::HashMap::new();
                    for chunk in names_needing_lookup.chunks(50) {
                        let mut qb = sqlx::QueryBuilder::new(
                            "SELECT id, title FROM media WHERE kind = 'person' AND lower(title) IN (",
                        );
                        let mut sep = qb.separated(", ");
                        for name in chunk {
                            sep.push_bind(name.to_lowercase());
                        }
                        qb.push(")");
                        let rows: Vec<(Uuid, String)> = qb
                            .build_query_as()
                            .fetch_all(
                                &state
                                    .ctx
                                    .db,
                            )
                            .await
                            .unwrap_or_default();
                        for (pid, title) in rows {
                            map.insert(title.to_lowercase(), pid);
                        }
                    }
                    map
                };

            let mut person_medias: Vec<db::Media> = Vec::new();
            let mut person_rels: Vec<db::MediaRelation> = Vec::new();
            for (i, p) in people
                .iter()
                .enumerate()
            {
                let pid =
                    p.id.or_else(|| {
                        name_to_id
                            .get(
                                &p.name
                                    .to_lowercase(),
                            )
                            .copied()
                    })
                    .unwrap_or_else(|| {
                        crate::common::stable_media_uuid(
                            &db::MediaKind::Person,
                            &p.name
                                .to_lowercase(),
                        )
                    });
                let role = p
                    .type_
                    .as_deref()
                    .map(|t| match t {
                        "Director" => db::RelationRole::Director,
                        "Writer" => db::RelationRole::Writer,
                        "Producer" => db::RelationRole::Producer,
                        "Creator" => db::RelationRole::Creator,
                        _ => db::RelationRole::Actor,
                    });
                // Only push a media stub for truly new persons (no existing record).
                if p.id
                    .is_none()
                    && !name_to_id.contains_key(
                        &p.name
                            .to_lowercase(),
                    )
                {
                    person_medias.push(db::Media {
                        id: pid,
                        title: p
                            .name
                            .clone(),
                        kind: db::MediaKind::Person,
                        ..Default::default()
                    });
                }
                person_rels.push(db::MediaRelation {
                    left_media_id: id,
                    right_media_id: pid,
                    weight: Some(i as i64),
                    role,
                    character: p
                        .role
                        .clone(),
                    ..Default::default()
                });
            }
            if !person_medias.is_empty() {
                db::Media::upsert(
                    &state
                        .ctx
                        .db,
                    &person_medias,
                )
                .await
                .inspect_err(|e| warn!(error = %e, "failed to upsert person media"))
                .ok();
            }
            db::MediaRelation::upsert(
                &state
                    .ctx
                    .db,
                &person_rels,
            )
            .await
            .inspect_err(|e| warn!(error = %e, "failed to upsert person relations"))
            .ok();
        }
    }

    Ok(StatusCode::NO_CONTENT)
}

#[query]
#[derive(Debug, Default)]
struct ContentTypeQuery {
    content_type: Option<String>,
}

#[post("/items/{id}/contenttype")]
pub async fn update_item_content_type(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Query(q): Query<ContentTypeQuery>,
) -> Result<StatusCode> {
    let raw = q
        .content_type
        .unwrap_or_default();
    info!(id = %id, content_type = %raw, "update_item_content_type");
    if raw.is_empty() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let kind = db::MediaKind::try_from(raw.as_str())
        .or_else(|_| db::MediaKind::try_from(raw.to_lowercase()))
        .map_err(|_| anyhow::anyhow!("invalid content type: {raw}"))
        .context_bad_request("Invalid content type")?;
    sqlx::query("UPDATE media SET kind = ? WHERE id = ?")
        .bind(kind.to_string())
        .bind(id)
        .execute(
            &state
                .ctx
                .db,
        )
        .await?;
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
    sort_order: Option<i64>,
    latest_auto_unplayed: Option<bool>,
    latest_sort_digital: Option<bool>,
    collection_default_sort: Option<Vec<api::ItemSortBy>>,
    collection_default_sort_order: Option<Vec<api::SortOrder>>,
}

#[patch("/items/{id}")]
pub async fn patch_item(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<PatchItemRequest>,
) -> Result<StatusCode> {
    if payload.latest_auto_unplayed == Some(true)
        || payload.latest_sort_digital == Some(true)
    {
        let effective_kind = if let Some(ct) = &payload.collection_type {
            parse_collection_type(ct)
        } else {
            let item = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &id,
            )
            .await?
            .ok_or_else(|| anyhow::anyhow!("item not found"))
            .context_not_found("Item not found")?;
            item.collection_media_kind
        };
        if effective_kind == Some(db::CollectionMediaKind::Collection) {
            return Err(anyhow::anyhow!(
                "latest_auto_unplayed and latest_sort_digital are not valid for group containers"
            ))
            .context_bad_request("Latest settings cannot be applied to group containers");
        }
    }

    let updated_at = Utc::now().naive_utc();
    let mut qb = sqlx::QueryBuilder::new("UPDATE media SET updated_at = ");
    qb.push_bind(updated_at);

    if let Some(name) = &payload.name {
        qb.push(", title = ")
            .push_bind(name);
    }
    if let Some(ct) = &payload.collection_type {
        let media_kind = parse_collection_type(ct);
        {
            let parsed_kind = payload
                .collection_kind
                .as_deref()
                .and_then(|s| db::CollectionKind::try_from(s).ok());
            require_valid_group_kind(media_kind.as_ref(), parsed_kind.as_ref())?;
        }
        qb.push(", collection_media_kind = ")
            .push_bind(
                media_kind
                    .as_ref()
                    .map(|k| k.to_string()),
            );
    }
    if let Some(ck) = &payload.collection_kind {
        qb.push(", collection_kind = ")
            .push_bind(ck);
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
    if let Some(so) = payload.sort_order {
        qb.push(", sort_order = ")
            .push_bind(so);
    }
    if let Some(v) = payload.latest_auto_unplayed {
        qb.push(", collection_latest_auto_unplayed = ")
            .push_bind(v);
    }
    if let Some(v) = payload.latest_sort_digital {
        qb.push(", collection_latest_sort_digital = ")
            .push_bind(v);
    }
    if let Some(ref v) = payload.collection_default_sort {
        qb.push(", collection_default_sort = ")
            .push_bind(sqlx::types::Json(v));
    }
    if let Some(ref v) = payload.collection_default_sort_order {
        qb.push(", collection_default_sort_order = ")
            .push_bind(sqlx::types::Json(v));
    }

    qb.push(" WHERE id = ")
        .push_bind(id);
    qb.build()
        .execute(
            &state
                .ctx
                .db,
        )
        .await?;

    if let Some(tags) = &payload.tags {
        set_tags(
            &state
                .ctx
                .db,
            id,
            tags,
        )
        .await
        .context_bad_request("Failed to update tags")?;
    }

    Ok(StatusCode::NO_CONTENT)
}

fn warm_providers_cache(ctx: &crate::AppContext, media: &db::Media) {
    let mut media = media.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let _ = ctx
            .addons
            .fetch_subtitles(&mut media, &ctx.db, true, None)
            .await;
        let _ = media
            .grandparent(&ctx.db)
            .await;
        let _ = ctx
            .addons
            .fetch_segments(&media, &ctx, true)
            .await;
    });
}

#[query]
#[derive(Default)]
pub struct SegmentQuery {
    #[serde(rename = "includeSegmentTypes", default)]
    include_segment_types: Vec<remux_sdks::remux::MediaSegmentType>,
}

fn segments_to_dtos(
    item_id: Uuid,
    source_id: Uuid,
    segs: &remux_sdks::remux::MediaSegments,
    type_filter: Option<&[remux_sdks::remux::MediaSegmentType]>,
) -> Vec<remux_sdks::remux::MediaSegmentDto> {
    use remux_sdks::remux::MediaSegmentDto;
    use uuid::Uuid;

    segs.to_pairs()
        .into_iter()
        .filter(|(t, _)| type_filter.map_or(true, |f| f.contains(t)))
        .map(|(t, seg)| {
            // Derive a stable UUID from (source_id, type discriminant).
            let mut bytes = [0u8; 16];
            let src = source_id.as_bytes();
            for (i, b) in src
                .iter()
                .enumerate()
            {
                bytes[i] ^= b;
            }
            bytes[15] ^= t as u8;
            MediaSegmentDto {
                id: Uuid::from_bytes(bytes),
                item_id,
                r#type: t,
                start_ticks: seg.start_ticks,
                end_ticks: seg.end_ticks,
            }
        })
        .collect()
}

#[get("/mediasegments/{id}")]
pub async fn media_segments(
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<SegmentQuery>,
    State(state): State<crate::AppState>,
) -> Result<impl IntoResponse> {
    let type_filter = if q
        .include_segment_types
        .is_empty()
    {
        None
    } else {
        Some(q.include_segment_types)
    };
    let filter_ref = type_filter.as_deref();

    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .unwrap_or_else(|| db::Media {
        id,
        ..Default::default()
    });
    let _ = media
        .grandparent(
            &state
                .ctx
                .db,
        )
        .await;

    let segs = state
        .ctx
        .addons
        .fetch_segments(&media, &state.ctx, false)
        .await;
    let dtos = segments_to_dtos(id, id, &segs, filter_ref);

    let count = dtos.len();
    Ok(Json(serde_json::json!({
        "Items": dtos,
        "TotalRecordCount": count,
        "StartIndex": 0,
    })))
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use http::header::HeaderValue;
    use remux_sdks::remux::{
        CollectionFilter, FilterGroup, FilterMatchMode, FilterRule, SetOp,
    };
    use uuid::Uuid;

    use crate::{
        db,
        db::{ExternalIds, MediaIdRaw, NonEmptyString},
        integration_test::{
            assert_api_keys_are_real, auth_header_with_token, authenticated_server,
            insert_test_source_of_kind,
        },
    };

    async fn get_user_id(server: &axum_test::TestServer, auth: &str) -> String {
        let resp: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(auth).unwrap(),
            )
            .await
            .json();
        resp["Id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn tag_filter(tag: &str) -> CollectionFilter {
        CollectionFilter {
            match_mode: FilterMatchMode::All,
            groups: vec![FilterGroup {
                match_mode: FilterMatchMode::All,
                rules: vec![FilterRule::Tag {
                    op: SetOp::In,
                    values: vec![tag.to_string()],
                }],
            }],
        }
    }

    async fn insert_smart_collection_with_filter(
        db: &sqlx::SqlitePool,
        title: &str,
        media_kind: db::CollectionMediaKind,
        filter: Option<CollectionFilter>,
    ) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            collection_media_kind: Some(media_kind),
            collection_smart_filter: filter,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .unwrap();
        c
    }

    fn make_content_ids(kind: db::MediaKind, imdb: &str) -> (Uuid, ExternalIds) {
        let ext = ExternalIds {
            imdb: Some(NonEmptyString::try_new(imdb.to_string()).unwrap()),
            ..Default::default()
        };
        let id = Uuid::from(&MediaIdRaw {
            kind: kind.clone(),
            external_ids: ext.clone(),
            season: None,
            episode: None,
        });
        (id, ext)
    }

    async fn insert_media(
        db: &sqlx::SqlitePool,
        title: &str,
        kind: db::MediaKind,
        imdb: &str,
    ) -> db::Media {
        let now = Utc::now().naive_utc();
        let (id, ext) = make_content_ids(kind.clone(), imdb);
        let mut m = db::Media {
            id,
            title: title.to_string(),
            kind,
            external_ids: ext,
            created_at: now,
            updated_at: now,
            released_at: Some(now - chrono::Duration::days(365)),
            ..Default::default()
        };
        m.save(db)
            .await
            .expect("insert_media failed");
        m
    }

    async fn insert_smart_collection(
        db: &sqlx::SqlitePool,
        title: &str,
        media_kind: db::CollectionMediaKind,
    ) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            collection_media_kind: Some(media_kind),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_smart_collection failed");
        c
    }

    // Requests movies from a series-only smart collection; must return nothing.
    #[tokio::test]
    async fn test_include_item_types_mismatched_returns_empty() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let collection =
            insert_smart_collection(db, "Shows", db::CollectionMediaKind::Series).await;
        insert_media(db, "Breaking Bad", db::MediaKind::Series, "tt0903747").await;
        insert_media(db, "Inception", db::MediaKind::Movie, "tt1375666").await;

        let user: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = user["Id"]
            .as_str()
            .unwrap();

        let resp = server
            .get(&format!("/users/{}/items", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[
                (
                    "parentId",
                    collection
                        .id
                        .to_string()
                        .as_str(),
                ),
                ("includeItemTypes", "Movie"),
            ])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert_eq!(
            body["TotalRecordCount"], 0,
            "movie query on series collection must be empty"
        );
        assert_eq!(
            body["Items"]
                .as_array()
                .map(|a| a.len())
                .unwrap_or(0),
            0
        );
    }

    // Requests series from a series-only smart collection; must return the series.
    #[tokio::test]
    async fn test_include_item_types_matching_returns_items() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let collection =
            insert_smart_collection(db, "Shows", db::CollectionMediaKind::Series).await;
        insert_media(db, "Breaking Bad", db::MediaKind::Series, "tt0903747").await;

        let user: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = user["Id"]
            .as_str()
            .unwrap();

        let resp = server
            .get(&format!("/users/{}/items", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[
                (
                    "parentId",
                    collection
                        .id
                        .to_string()
                        .as_str(),
                ),
                ("includeItemTypes", "Series"),
            ])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["TotalRecordCount"]
                .as_i64()
                .unwrap_or(0)
                > 0,
            "series query on series collection must return items"
        );
    }

    // No includeItemTypes filter on a series collection should still return series.
    #[tokio::test]
    async fn test_no_include_item_types_returns_collection_default() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        let collection =
            insert_smart_collection(db, "Shows", db::CollectionMediaKind::Series).await;
        insert_media(db, "The Wire", db::MediaKind::Series, "tt0306414").await;

        let user: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = user["Id"]
            .as_str()
            .unwrap();

        let resp = server
            .get(&format!("/users/{}/items", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                collection
                    .id
                    .to_string()
                    .as_str(),
            )])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["TotalRecordCount"]
                .as_i64()
                .unwrap_or(0)
                > 0,
            "unfiltered query on series collection must return series"
        );
    }

    // /UserViews must not return a promoted smart collection with no matching content.
    #[tokio::test]
    async fn userviews_hides_empty_smart_collection() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: "Empty Provider Shows".to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            collection_media_kind: Some(db::CollectionMediaKind::Series),
            collection_smart_filter: Some(tag_filter("provider:NobodyHasThis")),
            promoted: true,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .unwrap();

        let body: serde_json::Value = server
            .get(&format!("/userviews?userId={user_id}"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();

        let empty = vec![];
        let names: Vec<&str> = body["Items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|i| i["Name"].as_str())
            .collect();

        assert!(
            !names.contains(&"Empty Provider Shows"),
            "empty smart collection must not appear in /UserViews; got: {names:?}"
        );
    }

    // Regression: GET /Items?parentId=<alias-uuid> used db::Media::get_by_id for the parent
    // lookup, which does a straight WHERE id = $1 and cannot follow alias mappings in the
    // in-memory store. Stremio addons expose series under a virtual alias UUID; children are
    // stored under the canonical UUID, so browsing returned empty. This test would have
    // returned TotalRecordCount=0 before the fix.
    #[tokio::test]
    async fn parent_id_alias_resolves_children() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;
        let now = Utc::now().naive_utc();

        let series =
            insert_media(db, "Alias Show", db::MediaKind::Series, "tt9000001").await;
        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series.id),
        );
        let mut season = db::Media {
            id: season_id,
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            parent_id: Some(series.id),
            grandparent_id: Some(series.id),
            idx: Some(1),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // Mimic a Stremio addon virtual alias: store alias_uuid -> canonical series.id.
        let alias_uuid = Uuid::new_v4();
        guard
            .0
            .store
            .save(
                alias_uuid.to_string(),
                series.id,
                std::time::Duration::from_secs(3600),
            );

        let resp = server
            .get(&format!("/users/{}/items", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                alias_uuid
                    .to_string()
                    .as_str(),
            )])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["TotalRecordCount"]
                .as_i64()
                .unwrap_or(0)
                > 0,
            "browsing a series via its alias UUID must return children; got TotalRecordCount={}",
            body["TotalRecordCount"]
        );
        assert!(
            body["Items"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .any(|i| i["Name"] == "Season 1")
                })
                .unwrap_or(false),
            "Season 1 must appear when browsing the series via its alias UUID"
        );
    }

    // Regression: GET /items/latest?parentId=<alias-uuid> used db::Media::get_by_id for the
    // parent lookup, so collection-level filters (IsUnplayed, sort) were silently skipped when
    // the parent arrived via an alias UUID. This test would return all items (not just unplayed)
    // before the fix.
    #[tokio::test]
    async fn items_latest_alias_applies_collection_filters() {
        use crate::db::auth;

        let (server, guard, token) = authenticated_server().await;
        let auth_hdr = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth_hdr).await;
        let now = Utc::now().naive_utc();

        // A manual collection with auto-unplayed filtering enabled.
        let mut collection = db::Media {
            title: "Alias Collection".to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Manual),
            collection_latest_auto_unplayed: Some(true),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        collection
            .save(db)
            .await
            .unwrap();

        // Two movies in the collection.
        let movie_a =
            insert_media(db, "Movie A", db::MediaKind::Movie, "tt8000001").await;
        let movie_b =
            insert_media(db, "Movie B", db::MediaKind::Movie, "tt8000002").await;
        db::MediaRelation::add_collection_items(
            db,
            &collection.id,
            &[movie_a.id, movie_b.id],
        )
        .await
        .unwrap();

        // Mark Movie A as played.
        let user: db::User = sqlx::query_as("SELECT * FROM users LIMIT 1")
            .fetch_one(db)
            .await
            .unwrap();
        let mut ms = db::UserMediaState::get_or_new(db, &user, &movie_a)
            .await
            .unwrap();
        ms.play_count = 1;
        ms.save(db)
            .await
            .unwrap();

        // Store alias_uuid -> canonical collection.id.
        let alias_uuid = Uuid::new_v4();
        guard
            .0
            .store
            .save(
                alias_uuid.to_string(),
                collection.id,
                std::time::Duration::from_secs(3600),
            );

        let resp = server
            .get("/items/latest")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_hdr).unwrap(),
            )
            .add_query_params(&[
                (
                    "parentId",
                    alias_uuid
                        .to_string()
                        .as_str(),
                ),
                ("userId", user_id.as_str()),
            ])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let items = body
            .as_array()
            .unwrap();
        assert_eq!(
            items.len(),
            1,
            "IsUnplayed filter must be applied via alias UUID; expected only Movie B, got: {:?}",
            items
                .iter()
                .map(|i| &i["Name"])
                .collect::<Vec<_>>()
        );
        assert_eq!(items[0]["Name"], "Movie B");
    }

    // Regression: resolve_item's fast path handles Uuid aliases (store has alias→canonical Uuid).
    // This test covers the persist_from_store path where the store holds a db::Media object
    // (not yet a Uuid alias) — i.e. the very first time a Stremio addon series is browsed
    // before any alias mapping has been established.
    #[tokio::test]
    async fn parent_id_resolves_via_persist_from_store() {
        let (server, guard, token) = authenticated_server().await;
        let auth_hdr = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth_hdr).await;
        let now = Utc::now().naive_utc();

        // Build the canonical series (IMDB-derived UUID) and insert it into DB.
        let (canonical_id, series_ext) =
            make_content_ids(db::MediaKind::Series, "tt9000002");
        let mut series = db::Media {
            id: canonical_id,
            title: "Store Persist Show".to_string(),
            kind: db::MediaKind::Series,
            external_ids: series_ext.clone(),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        // Season linked to the canonical series UUID.
        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", canonical_id),
        );
        let mut season = db::Media {
            id: season_id,
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            parent_id: Some(canonical_id),
            grandparent_id: Some(canonical_id),
            idx: Some(1),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // Mimic a Stremio addon first encounter: store a db::Media under a synthetic UUID.
        // persist_from_store will recompute the canonical UUID from the IMDB ID, find it in DB,
        // and return it — without any Uuid alias pre-existing in the store.
        let alias_uuid = Uuid::new_v4();
        let stub = db::Media {
            id: alias_uuid,
            title: "Store Persist Show".to_string(),
            kind: db::MediaKind::Series,
            external_ids: series_ext,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        guard
            .0
            .store
            .save(
                alias_uuid.to_string(),
                stub,
                std::time::Duration::from_secs(3600),
            );

        let resp = server
            .get(&format!("/users/{}/items", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_hdr).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                alias_uuid
                    .to_string()
                    .as_str(),
            )])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["TotalRecordCount"]
                .as_i64()
                .unwrap_or(0)
                > 0,
            "browsing via a store-persisted db::Media alias must return children; got TotalRecordCount={}",
            body["TotalRecordCount"]
        );
        assert!(
            body["Items"]
                .as_array()
                .map(|a| a
                    .iter()
                    .any(|i| i["Name"] == "Season 1"))
                .unwrap_or(false),
            "Season 1 must appear when browsing via a db::Media alias in the store"
        );
    }

    /// Build a Movie row carrying probe data with French (index 2) and English
    /// (index 3) subtitle tracks. A fresh `streams_refreshed_at` makes
    /// `refresh_streams` short-circuit, so the detail page serves the probe data
    /// directly — mirroring `playback::tests::insert_subtitle_source`.
    async fn insert_subtitle_movie(ctx: &crate::AppContext) -> db::Media {
        let now = Utc::now().naive_utc();
        let probe = crate::api::MediaSourceInfo {
            container: Some("mp4".to_string()),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                crate::api::MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(crate::api::MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                crate::api::MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(crate::api::MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
                crate::api::MediaStream {
                    codec: Some("subrip".to_string()),
                    type_: Some(crate::api::MediaStreamType::Subtitle),
                    index: 2,
                    language: Some("fra".to_string()),
                    is_text_subtitle_stream: true,
                    ..Default::default()
                },
                crate::api::MediaStream {
                    codec: Some("subrip".to_string()),
                    type_: Some(crate::api::MediaStreamType::Subtitle),
                    index: 3,
                    language: Some("eng".to_string()),
                    is_text_subtitle_stream: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let (id, ext) =
            make_content_ids(db::MediaKind::Movie, "tt-sub-fallback-detail");
        let mut media = db::Media {
            id,
            title: "Subtitle Fallback Detail Test".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: ext,
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-subs-detail.mkv".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(probe),
            streams_refreshed_at: Some(now),
            created_at: now,
            updated_at: now,
            released_at: Some(now - chrono::Duration::days(365)),
            ..Default::default()
        };
        media
            .save(&ctx.db)
            .await
            .expect("insert_subtitle_movie failed");
        media
    }

    /// The Items endpoint (detail page) must apply the server's global
    /// preferred_metadata_language as a subtitle fallback when the user has no
    /// subtitle language preference.
    #[tokio::test]
    async fn test_items_detail_applies_server_metadata_language_subtitle_fallback() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let media = insert_subtitle_movie(ctx).await;

        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: Some("fr".to_string()),
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let resp = server
            .get(&format!("/items/{}", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("Fields", "MediaSources")])
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert_eq!(
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].as_i64(),
            Some(2),
            "detail page should fall back to server preferred_metadata_language 'fr' (French subtitle, index 2) when the user has no subtitle language preference"
        );
    }

    async fn insert_group_container(
        db: &sqlx::SqlitePool,
        title: &str,
        promoted: bool,
    ) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Manual),
            collection_media_kind: Some(db::CollectionMediaKind::Collection),
            promoted,
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_group_container failed");
        c
    }

    #[tokio::test]
    async fn group_container_returns_only_explicit_children() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        let group = insert_group_container(db, "TV Groups", false).await;
        let child_a =
            insert_smart_collection(db, "Netflix", db::CollectionMediaKind::Series)
                .await;
        let child_b =
            insert_smart_collection(db, "HBO", db::CollectionMediaKind::Series).await;
        let _unrelated =
            insert_smart_collection(db, "Disney", db::CollectionMediaKind::Movie).await;

        db::Media::set_parent_id(db, &[child_a.id, child_b.id], Some(group.id))
            .await
            .unwrap();

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                group
                    .id
                    .to_string()
                    .as_str(),
            )])
            .await
            .json();

        let empty = vec![];
        let names: Vec<&str> = body["Items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|i| i["Name"].as_str())
            .collect();

        assert!(
            names.contains(&"Netflix"),
            "child Netflix must appear; got: {names:?}"
        );
        assert!(
            names.contains(&"HBO"),
            "child HBO must appear; got: {names:?}"
        );
        assert!(
            !names.contains(&"Disney"),
            "unrelated Disney must not appear; got: {names:?}"
        );
        assert_eq!(
            names.len(),
            2,
            "only explicit children should appear; got: {names:?}"
        );
    }

    // patch_item must reject collection_type=collections when collection_kind is absent or non-manual.
    #[tokio::test]
    async fn patch_item_rejects_group_container_without_valid_kind() {
        use serde_json::json;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let now = Utc::now().naive_utc();

        let mut col = db::Media {
            title: "Test Col".to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        col.save(db)
            .await
            .unwrap();

        // No collection_kind → must 400
        server
            .patch(&format!("/items/{}", col.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "CollectionType": "collections" }))
            .expect_failure()
            .await
            .assert_status(http::StatusCode::BAD_REQUEST);

        // collection_kind=smart → allowed
        server
            .patch(&format!("/items/{}", col.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(
                &json!({ "CollectionType": "collections", "CollectionKind": "smart" }),
            )
            .await
            .assert_status(http::StatusCode::NO_CONTENT);

        // collection_kind=manual → allowed
        server
            .patch(&format!("/items/{}", col.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(
                &json!({ "CollectionType": "collections", "CollectionKind": "manual" }),
            )
            .await
            .assert_status(http::StatusCode::NO_CONTENT);
    }

    // update_virtual_folder must reject collection_type=collections when collection_kind is absent.
    #[tokio::test]
    async fn update_virtual_folder_rejects_group_container_without_valid_kind() {
        use serde_json::json;

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let now = Utc::now().naive_utc();

        let mut col = db::Media {
            title: "VF Test".to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        col.save(db)
            .await
            .unwrap();

        // No collection_kind → must 400 (was silently accepted before fix)
        server
            .post("/library/virtualfolders/LibraryOptions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "Id": col.id, "Name": "VF Test", "CollectionType": "collections" }))
            .expect_failure()
            .await
            .assert_status(http::StatusCode::BAD_REQUEST);

        // collection_kind=manual → must succeed
        server
            .post("/library/virtualfolders/LibraryOptions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "Id": col.id, "Name": "VF Test", "CollectionType": "collections", "CollectionKind": "manual" }))
            .await
            .assert_status(http::StatusCode::NO_CONTENT);
    }

    async fn insert_smart_group_container(
        db: &sqlx::SqlitePool,
        title: &str,
        filter: CollectionFilter,
    ) -> db::Media {
        let now = Utc::now().naive_utc();
        let mut c = db::Media {
            title: title.to_string(),
            kind: db::MediaKind::Collection,
            collection_kind: Some(db::CollectionKind::Smart),
            collection_media_kind: Some(db::CollectionMediaKind::Collection),
            collection_smart_filter: Some(filter),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        c.save(db)
            .await
            .expect("insert_smart_group_container failed");
        c
    }

    async fn tag_collection(db: &sqlx::SqlitePool, media_id: Uuid, tag: &str) {
        sqlx::query("INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)")
            .bind(media_id)
            .bind(tag)
            .execute(db)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn smart_group_container_returns_tag_matched_children() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        let group = insert_smart_group_container(
            db,
            "Sci-Fi Collections",
            tag_filter("genre:scifi"),
        )
        .await;

        let tagged_col = insert_smart_collection(
            db,
            "Sci-Fi Movies",
            db::CollectionMediaKind::Movie,
        )
        .await;
        let untagged_col = insert_smart_collection(
            db,
            "Comedy Movies",
            db::CollectionMediaKind::Movie,
        )
        .await;

        // Give each collection a child so they're not excluded by childless filter.
        let m1 = insert_media(db, "Alien", db::MediaKind::Movie, "tt0078748").await;
        let m2 =
            insert_media(db, "Dumb Movie", db::MediaKind::Movie, "tt0078749").await;
        db::MediaRelation::add_collection_items(db, &tagged_col.id, &[m1.id])
            .await
            .unwrap();
        db::MediaRelation::add_collection_items(db, &untagged_col.id, &[m2.id])
            .await
            .unwrap();

        tag_collection(db, tagged_col.id, "genre:scifi").await;

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[(
                "parentId",
                group
                    .id
                    .to_string()
                    .as_str(),
            )])
            .await
            .json();

        let empty = vec![];
        let names: Vec<&str> = body["Items"]
            .as_array()
            .unwrap_or(&empty)
            .iter()
            .filter_map(|i| i["Name"].as_str())
            .collect();

        assert!(
            names.contains(&"Sci-Fi Movies"),
            "tagged collection must appear in smart group; got: {names:?}"
        );
        assert!(
            !names.contains(&"Comedy Movies"),
            "untagged collection must not appear in smart group; got: {names:?}"
        );
    }

    /// Android TV refetches the item by MediaSource Id when the user picks a
    /// version, then plays `mediaSources.get(0)` because no source Id equals the
    /// item Id. The requested group's source must therefore come first.
    /// Regression test for issue #220.
    #[tokio::test]
    async fn test_items_get_by_stream_group_id_puts_that_group_first() {
        use crate::api;
        use remux_sdks::remux::{
            StreamFilter, StreamQuality, StreamResolution, StreamRule,
        };

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let now = Utc::now().naive_utc();

        let web_group = db::StreamGroup::create(
            &ctx.db,
            "1080p · WEB",
            StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::WebDl, StreamQuality::WebRip],
                    },
                ],
            },
            0,
        )
        .await
        .unwrap();

        let bluray_group = db::StreamGroup::create(
            &ctx.db,
            "1080p · Blu-ray",
            StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::BluRay, StreamQuality::BluRayRemux],
                    },
                ],
            },
            1,
        )
        .await
        .unwrap();

        let make_probe = || api::MediaSourceInfo {
            container: Some("mkv".to_string()),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                api::MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(api::MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                api::MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(api::MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let movie =
            insert_media(&ctx.db, "Test Movie", db::MediaKind::Movie, "tt8888888")
                .await;
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(now)
            .bind(movie.id)
            .execute(&ctx.db)
            .await
            .unwrap();

        for (idx, filename) in [
            (0, "TestMovie.2026.1080p.WEB-DL.H264.mkv"),
            (1, "TestMovie.2026.1080p.BluRay.x264.mkv"),
        ] {
            let mut stream = db::Media {
                title: filename.to_string(),
                kind: db::MediaKind::Stream,
                parent_id: Some(movie.id),
                idx: Some(idx),
                stream_info: Some(crate::stream::StreamInfo {
                    descriptor: crate::stream::StreamDescriptor::Local(filename.into()),
                    filename: Some(filename.to_string()),
                    ..Default::default()
                }),
                probe_data: Some(make_probe()),
                created_at: now,
                updated_at: now,
                ..Default::default()
            };
            stream
                .save(&ctx.db)
                .await
                .unwrap();
        }

        // Loading the item first writes the `gitem:{user}:{group}` mapping that
        // /items/{group} needs, exactly like the real client flow.
        let resp = server
            .get(&format!("/items/{}", movie.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let sources = body["MediaSources"]
            .as_array()
            .unwrap();
        assert_eq!(sources.len(), 2, "expected one source per group");
        assert_eq!(
            sources[0]["Name"]
                .as_str()
                .unwrap(),
            "1080p · WEB"
        );
        assert_eq!(
            sources[0]["Id"]
                .as_str()
                .unwrap(),
            movie
                .id
                .simple()
                .to_string(),
            "source[0] keeps the item id in the default listing"
        );
        assert_eq!(
            sources[1]["Id"]
                .as_str()
                .unwrap(),
            bluray_group
                .id
                .simple()
                .to_string(),
        );

        let resp2 = server
            .get(&format!("/items/{}", bluray_group.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp2.assert_status_ok();
        let body2: serde_json::Value = resp2.json();
        let sources2 = body2["MediaSources"]
            .as_array()
            .unwrap();

        assert_eq!(
            sources2.len(),
            2,
            "the full version list must stay available"
        );
        assert_eq!(
            sources2[0]["Name"]
                .as_str()
                .unwrap(),
            "1080p · Blu-ray",
            "the requested group must be MediaSources[0] — Android TV plays get(0)"
        );
        assert_eq!(
            sources2[0]["Id"]
                .as_str()
                .unwrap(),
            bluray_group
                .id
                .simple()
                .to_string(),
            "the hoisted source must keep its group UUID, not the item id, so the \
             client sends the group back on PlaybackInfo"
        );
        assert_eq!(
            body2["Id"]
                .as_str()
                .unwrap(),
            movie
                .id
                .simple()
                .to_string(),
            "the item itself is still the parent movie"
        );
    }

    /// Tracks are wrapped as HLS sources here rather than in the playback layer,
    /// so this URL is built in its own place and needs its own check: it carries
    /// the session token the client must play with.
    #[tokio::test]
    async fn track_media_source_url_carries_the_real_token() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source_of_kind(&guard.0, db::MediaKind::Track).await;

        let resp = server
            .get(&format!("/items/{}", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        assert_api_keys_are_real(&resp.json::<serde_json::Value>(), &token);
    }

    /// Android TV calls `/items?parentId=<season>&startIndex=1` with no SortBy and
    /// no includeItemTypes. Episodes must come back in episode-number order regardless.
    #[tokio::test]
    async fn items_by_parent_season_sorted_by_episode_number() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;

        // Insert series → season → episodes in reverse order so insertion order
        // would give the wrong result without an explicit ORDER BY.
        let imdb = db::NonEmptyString::try_new("tt_ep_order_001".to_string()).unwrap();
        let series_id = uuid::Uuid::from(&db::MediaIdRaw {
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(imdb.clone()),
                ..Default::default()
            },
            season: None,
            episode: None,
        });
        let mut series = db::Media {
            id: series_id,
            title: "EpOrderSeries".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(imdb),
                ..Default::default()
            },
            ..Default::default()
        };
        series
            .save(db)
            .await
            .unwrap();

        let season_id = crate::common::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series_id),
        );
        let mut season = db::Media {
            id: season_id,
            title: "Season 1".to_string(),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series_id),
            parent_id: Some(series_id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(db)
            .await
            .unwrap();

        // Titles deliberately don't sort alphabetically in episode-number order so
        // a title-based sort (the old buggy path) produces a wrong result.
        let ep_titles = ["Zombie Attack", "Apple Pie", "Mango Dreams"];
        for (ep_num, title) in [3i64, 2, 1]
            .iter()
            .zip(
                ep_titles
                    .iter()
                    .rev(),
            )
        {
            let ep_num = *ep_num;
            let mut ep = db::Media {
                id: crate::common::stable_media_uuid(
                    &db::MediaKind::Episode,
                    &format!("{}:{ep_num}", season_id),
                ),
                title: title.to_string(),
                kind: db::MediaKind::Episode,
                grandparent_id: Some(series_id),
                parent_id: Some(season_id),
                parent_idx: Some(1),
                idx: Some(ep_num),
                ..Default::default()
            };
            ep.save(db)
                .await
                .unwrap();
        }

        // Reproduce the exact Android TV request: parentId only, no SortBy, no includeItemTypes.
        // Without includeItemTypes, `kinds` is empty — previously this triggered is_channel_query
        // via vacuous `.all()` truth, causing episodes to sort by channel_number/title instead of idx.
        let resp = server
            .get(&format!(
                "/items?startIndex=0&limit=100&parentId={}",
                season_id.simple()
            ))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let items = body["Items"]
            .as_array()
            .unwrap();

        assert_eq!(items.len(), 3, "expected 3 episodes");
        let indices: Vec<i64> = items
            .iter()
            .map(|i| {
                i["IndexNumber"]
                    .as_i64()
                    .unwrap()
            })
            .collect();
        // Titles sort alphabetically as Apple(2), Mango(3), Zombie(1) — wrong.
        // Correct is episode-number order: 1, 2, 3.
        assert_eq!(
            indices,
            vec![1, 2, 3],
            "episodes must be in episode-number order, got {indices:?}"
        );
    }
}
