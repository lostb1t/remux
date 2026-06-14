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
use remux_macros::{api_query, delete, get, patch, post};
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
                            || matches!(
                                s.first(),
                                Some(api::ItemSortBy::SortName)
                                    | Some(api::ItemSortBy::Name)
                            )
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

    let server_config = db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await
    .ok();
    let show_ungrouped = server_config
        .as_ref()
        .and_then(|c| c.stream_groups_show_ungrouped)
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
        let cfg = server_config
            .clone()
            .unwrap_or_default();

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
                            .map(api::db_media_to_item)
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
                    server_config.as_ref(),
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
                                .map(api::db_media_to_item),
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
                    let mut dto = api::db_media_to_item(media);
                    dto.playlist_item_id = Some(
                        rel.relation_id
                            .to_string(),
                    );
                    items.push(dto);
                }
            }
            return Ok(ItemsQueryResult {
                total_count: total,
                items,
            });
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
                            .is_none(),
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
                        ..Default::default()
                    },
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
                    server_config.as_ref(),
                    None,
                    Some(&parent),
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

            // Smart collections: items float freely (no parent_id constraint).
            q.parent_id = None;

            let media_kind_filter = if let Some(kind) = parent
                .collection_media_kind
                .clone()
            {
                match kind {
                    db::CollectionMediaKind::Movie => vec![db::MediaKind::Movie],
                    db::CollectionMediaKind::Series => vec![db::MediaKind::Series],
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
                        collection_types
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
                true,
                Some(&session.user),
                server_config.as_ref(),
                smart_filter,
                Some(&parent),
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
        server_config.as_ref(),
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
                server_config.as_ref(),
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
                session,
                ids[0],
                q.fields
                    .as_deref(),
            )
            .await?;
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
                                || matches!(
                                    s.first(),
                                    Some(api::ItemSortBy::SortName)
                                        | Some(api::ItemSortBy::Name)
                                )
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
    _state: State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

#[api_query]
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
    let server_config = db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await
    .ok();
    let show_ungrouped = server_config
        .as_ref()
        .and_then(|c| c.stream_groups_show_ungrouped)
        .unwrap_or(true);
    let mut media = match db::Media::get_by_filter(
        &state
            .ctx
            .db,
        &db::MediaFilter {
            id: Some(vec![id]),
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
    {
        Some(m) => m,
        None => match MediaResolveService::resolve_item(id, &state.ctx).await? {
            Some(m) => m,
            None => return Ok(None),
        },
    };

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
    if media.kind == db::MediaKind::Stream {
        media.sources = Some(vec![media.clone()]);
    } else if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode) {
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
    } else if media.kind == db::MediaKind::Track {
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
    let mut base_item = api::db_media_to_item(media.clone());

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
            run_time_ticks: media
                .runtime
                .and_then(|s| s.to_ticks(TickUnit::Seconds)),
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
    if media
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

    //let enable_subtitles_detail = crate::db::Settings::get_config(&state.ctx.db)
    //    .await
    //    .ok()
    //    .and_then(|c| c.enable_subtitles_detail)
    //    .unwrap_or(true);
    let enable_subtitles_detail = false;
    if enable_subtitles_detail {
        if let Some(ref mut sources) = base_item.media_sources {
            if !sources.is_empty() {
                let sub_langs = server_config
                    .as_ref()
                    .and_then(|c| {
                        c.subtitle_languages
                            .clone()
                    })
                    .unwrap_or_default();
                super::playback::inject_external_subtitles(
                    &state.ctx,
                    &media,
                    sources,
                    media.id,
                    &session
                        .device
                        .access_token,
                    sub_langs,
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
        .with_permissions(&session);
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
        .map(api::db_media_kind_to_collection_type);
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
        idx: payload.sort_order,
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
        "UPDATE media SET title = $1, promoted = $2, collection_media_kind = $3, collection_kind = $4, collection_max_items = $5, updated_at = $6, idx = $8 WHERE id = $7",
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

#[api_query]
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
            .map(api::db_media_to_item)
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
            .map(api::db_media_to_item)
            .collect(),
        total_record_count: result.total_count as i64,
        start_index: q
            .start_index
            .unwrap_or(0),
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

#[api_query]
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
        .map(api::db_media_to_item)
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
        qb.push(", idx = ")
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

/// Fire-and-forget: populate the 24-hour subtitle cache for a movie/episode so
/// Fire-and-forget: warm all external provider caches for a movie/episode.
/// Runs subtitles (addons) and segment data (IntroDb) in parallel inside a single task.
fn warm_providers_cache(ctx: &crate::AppContext, media: &db::Media) {
    let media = media.clone();
    let ctx = ctx.clone();
    tokio::spawn(async move {
        let _ = ctx
            .addons
            .fetch_subtitles(&media, &ctx.db)
            .await;
        let _ = ctx
            .addons
            .fetch_segments(&media, &ctx)
            .await;
    });
}

#[api_query]
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
        .fetch_segments(&media, &state.ctx)
        .await;
    let dtos = segments_to_dtos(id, id, &segs, filter_ref);

    let count = dtos.len();
    Ok(Json(serde_json::json!({
        "Items": dtos,
        "TotalRecordCount": count,
        "StartIndex": 0,
    })))
}
