use anyhow::Context;
use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::{delete, get, post};
use serde::Deserialize;
use axum_extra::extract::Query;
use http::StatusCode;
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
use chrono::Utc;
use chrono::Datelike;

use super::{mock_items, stub_json};

pub struct ItemsQueryResult {
    pub items: Vec<jellyfin::BaseItemDto>,
    pub total_count: i64,
}

pub async fn get_items(
    state: AppState,
    session: auth::AuthSession,
    mut q: jellyfin::GetItemsQuery,
    _count: bool,
) -> Result<ItemsQueryResult> {
    //trace!(?q, "get_items");
    let aio = session.aio
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?;

    let parent = if let Some(parent_id) = q.parent_id.clone() {
        db::Media::get_by_id(&state.ctx.db, &parent_id).await?
    } else {
        None
    };

    let search = q.search_term.clone().or(q.name_starts_with.clone());
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
            || ![jellyfin::MediaType::Movie, jellyfin::MediaType::Series, jellyfin::MediaType::Episode]
                .contains(&types[0])
        {
            return Ok(ItemsQueryResult {
                items: vec![],
                total_count: 0,
            });
        }

        if let Some(s) = search {
            // todo: need to to make parallel request for types

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

            //let items = vec![];
            return Ok(ItemsQueryResult {
                items: items,
                total_count: 9999,
            });
        }
        //  }
    }

    // if q.filters.is_some() {
    //     return Ok(ItemsQueryResult {
    //         items: vec![],
    //         total_count: 0,
    //     });
    // }

    let requested = q.get_requested_item_types();
    if requested.iter().any(|t| matches!(t, jellyfin::MediaType::BoxSet | jellyfin::MediaType::CollectionFolder)) {
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
            items: records.into_iter().map(jellyfin::db_media_to_item).collect(),
        });
    }

    let manifest = aio.get_manifest().await?;

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
                items: result.records.into_iter().map(jellyfin::db_media_to_item).collect(),
            });
        }

        // catalog get
        if parent.kind == db::MediaKind::Collection {
            q.parent_id = None;

            if let Some(kind) = parent.collection_media_kind.clone() {
                q.include_item_types = Some(vec![jellyfin::db_media_kind_to_type(kind)]);
            } else {
                q.include_item_types = Some(vec![
                    jellyfin::MediaType::Movie,
                    jellyfin::MediaType::Series,
                ]);
            }
            //             q.include_item_types = Some(vec![jellyfin::MediaType::Movie]);
            // trace!(?q, "CATALOG");
            if q.limit.is_none() {
                q.limit = Some(250);
            }

            q.user_id = Some(session.user.id.clone());

            let mut result =
                db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, true).await?;

            return Ok(ItemsQueryResult {
                total_count: result.total_count as i64,
                items: result.records.into_iter().map(jellyfin::db_media_to_item).collect(),
            });
        }

        //  }
    }
    //trace!(?q, "get_items");
    let mut result =
        db::Media::get_by_jellyfin_filter(&state.ctx.db, &q, false).await?;

    // handle details request
    if let Some(ids) = &q.ids {
        if ids.len() == 1 {
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
                    if let Some(aio) = state.ctx.aio.as_ref() {
                        media.refresh_sources(&state.ctx.db, aio).await?;
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

    Ok(ItemsQueryResult {
        items: result.records.into_iter().map(jellyfin::db_media_to_item).collect(),
        total_count: 999_999,
    })
}

#[get("/items/latest")]
pub async fn items_flat(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    let items = get_items(state, session, q, false).await?;
    Ok(Json::<Vec<jellyfin::BaseItemDto>>(items.items))
}

#[get("/items")]
pub async fn items(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    //trace!(?q);
    let items = get_items(state, session, q.clone(), true).await?;

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        total_record_count: items.total_count as i64,
        start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

/// Get item counts
#[get("/items/counts")]
pub async fn items_counts(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    // Get counts for different media types from the database
    let movie_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Movie).await? as i32;
    let series_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Series).await? as i32;
    let episode_count = db::Media::count_by_kind(&state.ctx.db, &db::MediaKind::Episode).await? as i32;
    
    // For now, return hardcoded values for other types since we don't have them in the database yet
    // In a real implementation, you would query the actual counts
    let item_counts = jellyfin::ItemCounts {
        movie_count,
        series_count,
        episode_count,
        artist_count: 0, // TODO: Implement artist counting
        program_count: 0, // TODO: Implement program counting
        trailer_count: 0, // TODO: Implement trailer counting
        song_count: 0, // TODO: Implement song counting
        album_count: 0, // TODO: Implement album counting
        music_video_count: 0, // TODO: Implement music video counting
        box_set_count: 0, // TODO: Implement box set counting
        book_count: 0, // TODO: Implement book counting
        item_count: movie_count + series_count + episode_count, // Total of counted items
    };

    Ok(Json(item_counts))
}

pub async fn item(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
) -> Result<Option<jellyfin::BaseItemDto>> {
    let manifest = session.aio.as_ref()
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?
        .get_manifest().await?;
    // let libraries = super::get_virtual_folders(&state).await?;

    // if let Some(library) = libraries.into_iter().find(|x| x.id == id) {
    //     return Ok(Some(library));
    // }

    let q = jellyfin::GetItemsQuery {
        ids: vec![id].into(),
        ..Default::default()
    };
    return Ok(get_items(state, session, q, false)
        .await?
        .items
        .first()
        .cloned());
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
pub async fn persons(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: vec![],
        ..Default::default()
    }))
}

#[get("/items/filters")]
pub async fn items_filters(State(state): State<AppState>) -> Result<impl IntoResponse> {
    /// genres is actually tags?
    use strum::IntoEnumIterator;
    // let genres = db::Genre::iter().map(|g| g.to_string()).collect();
    let current_year = chrono::Utc::now().year() as i64;
    //let years = (1900..=current_year).collect();

    Ok(Json(jellyfin::QueryFiltersLegacy {
        //  genres: Some(genres),
        //  years: Some(years),
        genres: None,
        years: None,
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
    collection_max_items: Option<i64>,
}

#[post("/library/virtualfolders")]
pub async fn create_virtual_folder(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(payload): Json<VirtualFolderRequest>,
) -> Result<Json<jellyfin::VirtualFolderInfo>> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("forbidden")).context_unauthorized("Forbidden", "Forbidden");
    }

    let collection_media_kind = payload
        .collection_type
        .as_deref()
        .and_then(|s| parse_collection_type(s));

    let collection_kind = payload
        .collection_kind
        .as_deref()
        .and_then(|s| db::CollectionKind::try_from(s).ok())
        .unwrap_or(db::CollectionKind::Smart);

    let promoted: i64 = if payload.promoted.unwrap_or(false) { 1 } else { 0 };

    let mut media = db::Media {
        title: payload.name,
        kind: db::MediaKind::Collection,
        collection_kind: Some(collection_kind),
        collection_media_kind,
        collection_max_items: payload.collection_max_items,
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
    session: auth::AuthSession,
    Json(payload): Json<UpdateVirtualFolderRequest>,
) -> Result<StatusCode> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("forbidden")).context_unauthorized("Forbidden", "Forbidden");
    }

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

    let promoted: i64 = if payload.promoted.unwrap_or(false) { 1 } else { 0 };
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
    session: auth::AuthSession,
    Query(q): Query<DeleteVirtualFolderQuery>,
) -> Result<StatusCode> {
    if !session.user.is_admin {
        return Err(anyhow::anyhow!("forbidden")).context_unauthorized("Forbidden", "Forbidden");
    }

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
    Path(_id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::MetadataEditorInfo::default()))
}

#[get("/items/{id}/similar")]
pub async fn items_similar(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[get("/items/{id}/thememedia")]
pub async fn items_thememedia(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub_json(State(state)).await
}

#[get("/channels")]
pub async fn channels(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}
