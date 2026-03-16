use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::{delete, get, patch, post};
use serde::Deserialize;
use std::time::Duration;
use tracing::trace;
use tracing::warn;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use crate::sdks;
use crate::utils::IntoVec;
use axum_anyhow::{ApiResult as Result, IntoApiError, OptionExt, ResultExt};
use sqlx::SqlitePool;
use chrono::Datelike;
use chrono::Utc;

use super::{mock_items, stub_json};

pub struct ItemsQueryResult {
    pub items: Vec<jellyfin::BaseItemDto>,
    pub total_count: i64,
}

impl ItemsQueryResult {
    pub fn with_permissions(mut self, session: &auth::AuthSession) -> Self {
        for item in &mut self.items {
            item.can_delete = Some(db::Media::can_delete(&session.user));
        }
        self
    }
}

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: jellyfin::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    //trace!(?q, "get_items");


    let parent = if let Some(parent_id) = q.parent_id.clone() {
        db::Media::get_by_id(&state.ctx.db, &parent_id).await?
    } else {
        None
    };

    //let search = q.search_term.clone().or(q.name_starts_with.clone());
    let search = q.search_term.clone();
    let skip = q.start_index.unwrap_or(0) as u32;

    //  trace!(?q, "get_items");

    // only support Movie, Series, and Episode for search and catalogs
    if search.is_some()
        || parent
            .clone()
            .map_or(false, |p| p.kind == db::MediaKind::Collection)
    {
        let types = q.get_requested_item_types();
        // if types.len() != 0 {
        if types.len() == 0
            || ![
                jellyfin::MediaType::Movie,
                jellyfin::MediaType::Series,
                jellyfin::MediaType::Episode,
            ]
            .contains(&types[0])
        {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        if let Some(s) = search {
            // todo: need to to make parallel request for types
    if let Ok(aio) = crate::aio::AioService::from_settings(&state.ctx.db)
        .await
{
            let items = aio
                .search(types[0].clone().into(), s)
                .await?
                .into_iter()
                .filter_map(|meta| match db::Media::try_from(meta.clone()) {
                    Ok(media) => {
                        state.ctx.store.save(
                            media.id.clone(),
                            meta.clone(),
                            Duration::from_secs(360),
                        );
                        Some(jellyfin::db_media_to_item(media))
                    }
                    Err(e) => {
                        warn!("Failed to convert item to Media: {}", e);
                        None
                    }
                })
                .collect::<Vec<_>>();

            return Ok(ItemsQueryResult {
                items: items,
                total_count: 9999,
            });
          
        } else {
          // fallthrough for jellyfin
          warn!("AIO not configured");
        }
         }
    }

    // if q.filters.is_some() {
    //     return Ok(ItemsQueryResult {
    //         items: vec![],
    //         total_count: 0,
    //     });
    // }

    let requested = q.get_requested_item_types();
    if requested.iter().any(|t| {
        matches!(
            t,
            jellyfin::MediaType::BoxSet | jellyfin::MediaType::CollectionFolder
        )
    }) {
        let records = db::Media::get_by_filter(
            &state.ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Collection]),
                ..Default::default()
            },
        )
        .await?
        .records;
        return Ok(ItemsQueryResult {
            total_count: records.len() as i64,
            items: records
                .into_iter()
                .map(jellyfin::db_media_to_item)
                .collect(),
        });
    }

    //let manifest = aio.get_manifest().await?;

    if let Some(parent) = &parent {
        if parent.id == db::collection_uuid() {
            let result = db::Media::get_by_filter(
                &state.ctx.db,
                &db::MediaFilter {
                    kind: Some(vec![db::MediaKind::Collection]),
                    //promoted: Some(true),
                    ..Default::default()
                },
            )
            .await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result
                    .records
                    .into_iter()
                    .map(jellyfin::db_media_to_item)
                    .collect(),
            });
        }

        // collection browse
        if parent.kind == db::MediaKind::Collection {
            // All collection types: items float freely (no parent_id constraint).
            q.parent_id = None;

            let media_kind_filter =
                if let Some(kind) = parent.collection_media_kind.clone() {
                    vec![kind]
                } else {
                    vec![db::MediaKind::Movie, db::MediaKind::Series]
                };

            q.include_item_types = Some(
                media_kind_filter
                    .iter()
                    .map(|k| jellyfin::db_media_kind_to_type(k.clone()))
                    .collect(),
            );

            if q.limit.is_none() {
                q.limit = Some(250);
            }
            q.user_id = Some(session.user.id.clone());

            // For smart collections with a catalog filter: query via media_relations
            let catalog_ids = parent.catalog_filter_ids();
            if parent.collection_kind == Some(db::CollectionKind::Smart)
                && !catalog_ids.is_empty()
            {
                let result = db::Media::get_by_filter(
                    &state.ctx.db,
                    &db::MediaFilter {
                        kind: Some(media_kind_filter),
                        catalog_ids: Some(catalog_ids),
                        limit: q.limit,
                        ..Default::default()
                    },
                )
                .await?;
                return Ok(ItemsQueryResult {
                    total_count: result.total_count as i64,
                    items: result
                        .records
                        .into_iter()
                        .map(jellyfin::db_media_to_item)
                        .collect(),
                });
            }

            let policy = session.user.policy.as_ref().map(|p| &p.0);
            let server_config = crate::db::Settings::get_config(&state.ctx.db).await.ok();
            let result =
                db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, true, policy, server_config.as_ref()).await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result
                    .records
                    .into_iter()
                    .map(jellyfin::db_media_to_item)
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
    let policy = session.user.policy.as_ref().map(|p| &p.0);
    let server_config = crate::db::Settings::get_config(&state.ctx.db).await.ok();
    //trace!(?q, "get_items");
    let mut result =
        db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, want_total, policy, server_config.as_ref()).await?;

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
              if let Ok(aio) = crate::aio::AioService::from_settings(&state.ctx.db)
        .await
{
            let mut media: Option<db::Media> =
                if let Some(media) = result.records.get(0) {
                    Some(media.clone())
                } else {
                    if let Some(meta) =
                        state.ctx.store.get::<sdks::aio::Meta>(*ids.get(0).unwrap())
                    {
                      
                        let mut media: db::Media = aio
                            .get_meta(meta.media_type.clone(), meta.id.clone())
                            .await?
                            .try_into()?;

                        // web client makes 2 simultenious request. So we get race conditions.
                        if let Err(err) = media.save(&state.ctx.db).await {
                            media = db::Media::get_by_filter(
                                &state.ctx.db,
                                &db::MediaFilter {
                                    aio_id: media.aio_id.clone(),

                                    ..Default::default()
                                },
                            )
                            .await?
                            .records
                            .get(0)
                            .unwrap()
                            .clone();
                        }

                        Some(media)
                    } else {
                        None
                    }
                };
            if let Some(media) = media.as_mut() {
                if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
                    && (q.fields.is_none()
                        || q.fields.as_ref().map_or(false, |f| {
                            f.contains(&jellyfin::ItemFields::MediaSources)
                        }))
                {
                    if let Ok(aio) =
                        crate::aio::AioService::from_settings(&state.ctx.db).await
                    {
                        media.refresh_sources(&state.ctx.db, &aio).await?;
                    }
                    media.sources(&state.ctx.db).await?;
                    // always load state for single
                    media.user_state(&state.ctx.db, &session.user).await?;

                    if let Some(sources) = &media.sources {
                        trace!(streams_len = sources.len(), "sources");
                    }
                }

                media.load_relations(&state.ctx.db).await?;

                return Ok(ItemsQueryResult {
                    items: vec![jellyfin::db_media_to_item(media.clone())],
                    total_count: 1,
                });
            }
          }
        }
    }

    Ok(ItemsQueryResult {
        items: result
            .records
            .into_iter()
            .map(jellyfin::db_media_to_item)
            .collect(),
        total_count: result.total_count as i64,
    })
}

#[get("/items/latest")]
pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state.clone(), session.clone(), q, false).await?.with_permissions(&session);
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

#[get("/items")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state.clone(), session.clone(), q.clone(), true).await?.with_permissions(&session);

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
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
    Ok(Json(jellyfin::BaseItemDto {
        id: db::collection_uuid(),
        name: Some("Media Library".to_string()),
        type_: jellyfin::MediaType::CollectionFolder,
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
            .map(jellyfin::db_media_to_item)
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

/// Refresh a single item from AIO
#[post("/items/{id}/refresh")]
pub async fn refresh_item(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Path(id): Path<Uuid>,
) -> Result<StatusCode> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("Not Found", "Item not found")?;
    let aio_id = media
        .aio_id
        .as_deref()
        .context_bad_request("Bad Request", "Item has no AIO source")?;
    let (kind_str, item_id) = aio_id
        .split_once(':')
        .context_bad_request("Bad Request", "Invalid AIO id format")?;
    let media_type = match kind_str.to_lowercase().as_str() {
        "movie" => crate::sdks::aio::MediaType::Movie,
        "series" | "tv" => crate::sdks::aio::MediaType::Series,
        _ => {
            return Err(anyhow::anyhow!("Unknown AIO media type: {}", kind_str)
                .context_bad_request("Bad Request", "Unknown AIO media type"))
        }
    };
    let aio = crate::aio::AioService::from_settings(&state.ctx.db)
        .await
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?;
    let meta = aio.get_meta(media_type, item_id.to_string()).await?;
    let mut refreshed: db::Media = meta.try_into()?;
    refreshed.id = id;
    refreshed.save(&state.ctx.db).await?;
    let _ = state.ctx.ws_tx.send(crate::ws::WsEvent::LibraryChanged);
    Ok(StatusCode::NO_CONTENT)
}

/// Get filter options (genres + tags) for the modern /Items/Filters2 endpoint
#[get("/items/filters2")]
pub async fn items_filters2(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .map(db::MediaKind::from)
        .filter(|k| !matches!(k, db::MediaKind::Unknown))
        .collect();
    let genres = db::Media::get_genres(&state.ctx.db, &kinds).await?;
    let tag_rows = sqlx::query("SELECT DISTINCT tag FROM media_tags ORDER BY tag")
        .fetch_all(&state.ctx.db)
        .await?;
    Ok(Json(jellyfin::QueryFilters {
        genres: Some(
            genres
                .into_iter()
                .map(|g| jellyfin::NameIdPair {
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
    Ok(Json(Vec::<jellyfin::BaseItemDto>::new()))
}

#[get("/items/{id}/specialfeatures")]
pub async fn items_special_features(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<jellyfin::BaseItemDto>::new()))
}

#[get("/items/{id}/externalidinfos")]
pub async fn items_external_id_infos(
    _state: State<AppState>,
    _session: auth::AdminSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(Vec::<jellyfin::ExternalIdInfo>::new()))
}

#[get("/items/{id}/themevideos")]
pub async fn items_theme_videos(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult::default()))
}

#[get("/items/{id}/themesongs")]
pub async fn items_theme_songs(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult::default()))
}

#[get("/items/{id}/remoteimages")]
pub async fn items_remote_images(
    _state: State<AppState>,
    _session: auth::AuthSession,
    _path: Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::RemoteImageResult {
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
    // Get counts for different media types from the database
    let movie_count =
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Movie).await? as i32;
    let series_count =
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Series).await? as i32;
    let episode_count =
        db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Episode).await? as i32;

    // For now, return hardcoded values for other types since we don't have them in the database yet
    // In a real implementation, you would query the actual counts
    let item_counts = jellyfin::ItemCounts {
        movie_count,
        series_count,
        episode_count,
        artist_count: 0,      // TODO: Implement artist counting
        program_count: 0,     // TODO: Implement program counting
        trailer_count: 0,     // TODO: Implement trailer counting
        song_count: 0,        // TODO: Implement song counting
        album_count: 0,       // TODO: Implement album counting
        music_video_count: 0, // TODO: Implement music video counting
        box_set_count: 0,     // TODO: Implement box set counting
        book_count: 0,        // TODO: Implement book counting
        item_count: movie_count + series_count + episode_count, // Total of counted items
    };

    Ok(Json(item_counts))
}

pub async fn item(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
) -> Result<Option<jellyfin::BaseItemDto>> {
    let manifest = crate::aio::AioService::from_settings(&state.ctx.db)
        .await
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?
        .get_manifest()
        .await?;
    // let libraries = super::get_virtual_folders(&state).await?;

    // if let Some(library) = libraries.into_iter().find(|x| x.id == id) {
    //     return Ok(Some(library));
    // }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, session.clone(), q, false)
        .await?
        .with_permissions(&session)
        .items
        .into_iter()
        .next());
}

/// Jellyfin web requests `/Items/livetv` (literal string) when navigating to
/// the Live TV section — handle it before the `{id}` UUID route.
#[get("/items/livetv")]
pub async fn items_livetv(
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(super::shows::livetv_view_item()))
}

#[get("/items/{id}")]
pub async fn items_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    return Ok(Json(item(state, session, id).await?).into_response());
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
    //  jellyfin::BaseItemDto {
    //     name: c.title,
    //     ..Default::default()
    //   }
    //}
    //);
    //let tmdb_items = state.tmdb.movie_now_playing().send().await;
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/persons")]
pub async fn persons(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/items/filters")]
pub async fn items_filters(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .map(db::MediaKind::from)
        .filter(|k| !matches!(k, db::MediaKind::Unknown))
        .collect();

    let genres = db::Media::get_genres(&state.ctx.db, &kinds).await?;
    let years = db::Media::get_distinct_years(&state.ctx.db, &kinds).await?;

    Ok(Json(jellyfin::QueryFiltersLegacy {
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
    .map(|x| jellyfin::db_media_to_item(x))
    .collect::<Vec<_>>();

    let total = items.len() as i64;
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
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

fn media_to_virtual_folder(m: db::Media) -> jellyfin::VirtualFolderInfo {
    let collection_type = m
        .collection_media_kind
        .clone()
        .map(jellyfin::db_media_kind_to_collection_type);
    jellyfin::VirtualFolderInfo {
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
) -> Result<Json<jellyfin::VirtualFolderInfo>> {
    let collection_media_kind = payload
        .collection_type
        .as_deref()
        .and_then(|s| parse_collection_type(s));

    let collection_kind = payload
        .collection_kind
        .as_deref()
        .and_then(|s| db::CollectionKind::try_from(s).ok())
        .unwrap_or(db::CollectionKind::Smart);

    let promoted: i64 = if payload.promoted.unwrap_or(false) {
        1
    } else {
        0
    };

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

    let promoted: i64 = if payload.promoted.unwrap_or(false) {
        1
    } else {
        0
    };
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

    let catalogs: Vec<jellyfin::AioCatalogInfo> = manifest
        .catalogs
        .into_iter()
        .filter(|c| !c.id.contains("search"))
        .map(|c| {
            let aio_id = format!("{}:{}", c.kind, c.id);
            let db_cat = db_catalogs
                .iter()
                .find(|d| d.aio_id.as_deref() == Some(&aio_id));
            jellyfin::AioCatalogInfo {
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

fn parse_collection_type(s: &str) -> Option<db::MediaKind> {
    match s {
        "movies" => Some(db::MediaKind::Movie),
        "tvshows" => Some(db::MediaKind::Series),
        _ => None,
    }
}

#[get("/genres")]
pub async fn genres(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let related_kinds: Vec<db::MediaKind> = q
        .include_item_types
        .unwrap_or_default()
        .into_iter()
        .map(db::MediaKind::from)
        .filter(|k| !matches!(k, db::MediaKind::Unknown))
        .collect();

    let genres = db::Media::get_genres(&state.ctx.db, &related_kinds).await?;
    let total = genres.len() as i64;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: genres.into_iter().map(jellyfin::db_media_to_item).collect(),
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
    Ok(Json(jellyfin::MetadataEditorInfo::default()))
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

// ── set_tags helper ────────────────────────────────────────────────

async fn set_tags(db: &SqlitePool, id: Uuid, tags: &[String]) -> anyhow::Result<()> {
    sqlx::query("DELETE FROM media_tags WHERE media_id = ?")
        .bind(id)
        .execute(db)
        .await?;
    for tag in tags {
        sqlx::query(
            "INSERT OR IGNORE INTO media_tags (media_id, tag) VALUES (?, ?)",
        )
        .bind(id)
        .bind(tag)
        .execute(db)
        .await?;
    }
    Ok(())
}

// ── POST /items/{id} — Jellyfin web metadata editor ────────────────

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

// ── PATCH /items/{id} — partial update ────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct PatchItemRequest {
    name: Option<String>,
    collection_type: Option<String>,
    collection_kind: Option<String>,
    collection_catalog_filter: Option<Vec<String>>,
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
    if let Some(filter) = &payload.collection_catalog_filter {
        let json = serde_json::to_string(filter).unwrap_or_else(|_| "[]".into());
        qb.push(", collection_catalog_filter = ").push_bind(json);
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

// ── POST /aio/catalogs/{aio_id} — upsert catalog settings ─────────

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
    let promoted: i64 = if payload.enabled { 1 } else { 0 };
    let now = Utc::now().naive_utc();

    let existing = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Catalog]),
            aio_id: Some(aio_id.clone()),
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
        let title = payload.name.clone().unwrap_or_else(|| aio_id.clone());
        let mut media = db::Media {
            kind: db::MediaKind::Catalog,
            title,
            aio_id: Some(aio_id),
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
