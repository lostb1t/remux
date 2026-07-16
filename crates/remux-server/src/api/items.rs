use crate::services::{MediaResolveService, image::ImageService};
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

    pub fn with_client_patches(mut self) -> Self {
        let client = &self
            .session
            .device
            .app_name;
        self.hide_sources = client == "Plezy";
        self.mixed_collection_type = if client.contains("Swiftfin") {
            // Swiftfin's SDK has no "mixed" case; homevideos is accepted and shows a home row.
            Some(api::CollectionType::Homevideos)
        } else {
            None
        };
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

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
    want_count: bool,
) -> Result<ItemsQueryResultBuilder> {
    //trace!(?q, "get_items");
    if !want_count {
        q.enable_total_record_count = Some(false);
    }
    // Used only by pre-converting paths (search, playlist) that use with_dtos().
    // Raw-media paths delegate hide_sources to with_client_patches() on the builder.
    let hide_sources = session
        .device
        .app_name
        == "Plezy";

    let parent = if let Some(parent_id) = q
        .parent_id
        .clone()
    {
        db::Media::get_by_id(
            &state
                .ctx
                .db,
            &parent_id,
        )
        .await?
    } else {
        None
    };

    // Apply the collection's default sort override when the client sends no sort
    // preference or sends SortName as the primary sort (Jellyfin clients use
    // SortName as their generic default, so we treat it as "no preference").
    if let Some(ref p) = parent {
        if let Some(ref default_sort) = p.collection_default_sort {
            if !default_sort.is_empty() {
                let is_client_default = q
                    .sort_by
                    .as_deref()
                    .map(|s| {
                        s.is_empty()
                            || s.iter()
                                .any(|v| {
                                    matches!(
                                        v,
                                        api::ItemSortBy::SortName
                                            | api::ItemSortBy::Name
                                    )
                                })
                    })
                    .unwrap_or(true);
                if is_client_default {
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
                        .search(k, s, kind_limit(k, limit), &state.ctx)
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
            let mut items = Vec::with_capacity(slice.len());
            for rel in slice {
                if let Some(media) = db::Media::get_by_id(
                    &state
                        .ctx
                        .db,
                    &rel.right_media_id,
                )
                .await?
                {
                    let mut dto = api::db_media_to_item(media, hide_sources);
                    dto.playlist_item_id = Some(
                        rel.relation_id
                            .to_string(),
                    );
                    items.push(dto);
                }
            }
            return Ok(ItemsQueryResultBuilder::with_dtos(session, items, total));
        }

        // collection browse
        if parent.kind == db::MediaKind::Collection {
            // "Collections index": any collection with collection_media_kind='collection'
            // shows non-promoted collections regardless of collection_kind (manual/smart).
            if parent.collection_media_kind == Some(db::CollectionMediaKind::Collection)
            {
                let result = db::Media::get_by_filter(
                    &state
                        .ctx
                        .db,
                    &db::MediaFilter {
                        kind: Some(vec![db::MediaKind::Collection]),
                        promoted: Some(false),
                        limit: q.limit,
                        offset: q.start_index,
                        total_count: true,
                        include_user_state: q
                            .enable_user_data
                            .unwrap_or(true),
                        user_id: Some(
                            session
                                .user
                                .id,
                        ),
                        include_child_count: q
                            .fields
                            .as_deref()
                            .map(|f| f.contains(&api::ItemFields::ChildCount))
                            .unwrap_or(false),
                        sort_by: q
                            .sort_by
                            .clone()
                            .unwrap_or_default(),
                        sort_order: q
                            .sort_order
                            .clone()
                            .unwrap_or_default(),
                        exclude_childless: !q
                            .include_childless
                            .unwrap_or(false),
                        policy_filter: session
                            .user
                            .policy
                            .as_ref()
                            .and_then(|p| {
                                p.filter_rules
                                    .as_ref()
                            })
                            .cloned(),
                        ..Default::default()
                    },
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
            let smart_filter = if matches!(
                parent.collection_kind,
                Some(db::CollectionKind::Smart) | Some(db::CollectionKind::Catalog)
            ) {
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
    if let Some(parent_id) = q
        .parent_id
        .clone()
    {
        if let Ok(Some(parent)) = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &parent_id,
        )
        .await
        {
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
                    let is_client_default = q
                        .sort_by
                        .as_deref()
                        .map(|s| {
                            s.is_empty()
                                || s.iter()
                                    .any(|v| {
                                        matches!(
                                            v,
                                            api::ItemSortBy::SortName
                                                | api::ItemSortBy::Name
                                        )
                                    })
                        })
                        .unwrap_or(true);
                    if is_client_default {
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

#[get("/items")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
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
            .unwrap_or_else(|| 0),
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
        // Force-refresh streams by clearing the timestamp first.
        sqlx::query("UPDATE media SET streams_refreshed_at = NULL WHERE id = ?")
            .bind(media.id)
            .execute(
                &state
                    .ctx
                    .db,
            )
            .await
            .ok();

        state
            .ctx
            .addons
            .refresh_streams(&mut media, &state.ctx)
            .await
            .inspect_err(|e| error!("Could not refresh streams: {e:#}"));

        if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
            warm_providers_cache(&state.ctx, &media);
        }
    } else if q.metadata_refresh_mode == api::MetadataRefreshMode::FullRefresh {
        let force_refresh = q.replace_all_metadata;
        state
            .ctx
            .addons
            .process_meta_batch(vec![media], &state.ctx, force_refresh)
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
    let want_streams = fields
        .map(|f| f.contains(&api::ItemFields::MediaSources))
        .unwrap_or(true);
    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = server_config
        .stream_groups_show_ungrouped
        .unwrap_or(true);
    let resolved_id = match MediaResolveService::resolve_item(id, &state.ctx).await? {
        Some(m) if m.kind == db::MediaKind::StreamGroup => {
            // A StreamGroup UUID is a client-facing source ID, not a browsable item.
            // Redirect to the parent movie/episode so clients like Android TV land on
            // the actual content item instead of a bare StreamGroup record.
            match m.parent_id {
                Some(pid) => pid,
                None => return Ok(None),
            }
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
                session
                    .user
                    .id,
            ),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .context_not_found("item not found")?;

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
            .refresh_streams(&mut media, &state.ctx)
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
                p.media_streams
                    .clone()
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
            supports_transcoding: true,
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
    return Ok(Json(
        item(
            state,
            session,
            id,
            q.fields
                .as_deref(),
        )
        .await?,
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

    // Library name is baked into the generated placeholder — clear it so it regenerates.
    let _ = ImageService::delete_image(
        &state
            .ctx
            .config
            .data_dir,
        payload.id,
        db::ImageKind::Primary,
        &state
            .ctx
            .db,
    )
    .await;

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
    let updated_at = Utc::now().naive_utc();
    let mut qb = sqlx::QueryBuilder::new("UPDATE media SET updated_at = ");
    qb.push_bind(updated_at);

    if let Some(name) = &payload.name {
        qb.push(", title = ")
            .push_bind(name);
    }
    if let Some(ct) = &payload.collection_type {
        let media_kind = parse_collection_type(ct);
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
    let media = media.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let _ = ctx
            .addons
            .fetch_subtitles(&media, &ctx.db, true)
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

    let media = db::Media::get_by_id(
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
        integration_test::{auth_header_with_token, authenticated_server},
    };

    // The "Collections" container from seed data — shows non-promoted collections.
    const COLLECTIONS_PARENT_ID: &str = "f47ac10b-58cc-4372-a567-0e02b2c3d479";

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

    // Browsing the Collections container must not return smart collections whose
    // filter rules match nothing (e.g. Netflix tag with no Netflix content).
    #[tokio::test]
    async fn collections_parent_hides_empty_smart_collection() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        // A smart movie collection that requires a tag no movie in the DB has.
        insert_smart_collection_with_filter(
            db,
            "Top Provider Movies",
            db::CollectionMediaKind::Movie,
            Some(tag_filter("provider:NonExistent")),
        )
        .await;

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("parentId", COLLECTIONS_PARENT_ID)])
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
            !names.contains(&"Top Provider Movies"),
            "empty smart collection must not appear in Collections browse; got: {names:?}"
        );
    }

    // Browsing the Collections container must return smart collections that
    // do have matching content.
    #[tokio::test]
    async fn collections_parent_shows_non_empty_smart_collection() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let db = &guard
            .0
            .db;
        let user_id = get_user_id(&server, &auth).await;

        // Insert a movie tagged so the collection below matches it.
        let movie =
            insert_media(db, "Tagged Movie", db::MediaKind::Movie, "tt9991234").await;
        sqlx::query("INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)")
            .bind(movie.id)
            .bind("provider:TestNet")
            .execute(db)
            .await
            .unwrap();

        insert_smart_collection_with_filter(
            db,
            "TestNet Movies",
            db::CollectionMediaKind::Movie,
            Some(tag_filter("provider:TestNet")),
        )
        .await;

        let body: serde_json::Value = server
            .get(&format!("/users/{user_id}/items"))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .add_query_params(&[("parentId", COLLECTIONS_PARENT_ID)])
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
            names.contains(&"TestNet Movies"),
            "non-empty smart collection must appear in Collections browse; got: {names:?}"
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
}
