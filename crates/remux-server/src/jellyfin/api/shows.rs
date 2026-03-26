use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use http::StatusCode;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

use super::items::get_items;
use super::mock_items;

pub fn livetv_view_id() -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, b"remux-livetv-view")
}

pub fn livetv_view_item() -> jellyfin::BaseItemDto {
    jellyfin::BaseItemDto {
        id: livetv_view_id(),
        name: Some("Live TV".to_string()),
        type_: jellyfin::MediaType::CollectionFolder,
        collection_type: Some(jellyfin::CollectionType::Livetv),
        ..Default::default()
    }
}

#[get("/shows/{id}/seasons")]
pub async fn shows_seasons(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Season]);
    if q.sort_by.is_none() {
        q.sort_by = Some(vec![jellyfin::ItemSortBy::IndexNumber]);
        q.sort_order = Some(vec![jellyfin::SortOrder::Ascending]);
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        ..Default::default()
    }))
}

#[get("/shows/{id}/episodes")]
pub async fn shows_episodes(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    // q.season_id = Some(id);
    q.parent_id = q.season_id;
    q.include_item_types = Some(vec![jellyfin::MediaType::Episode]);
    if q.sort_by.is_none() {
        q.sort_by = Some(vec![jellyfin::ItemSortBy::IndexNumber]);
        q.sort_order = Some(vec![jellyfin::SortOrder::Ascending]);
    }
    let items = get_items(state, session.clone(), q.clone(), true)
        .await?
        .with_permissions(&session);

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items: items.items,
        // total_record_count: items.total_count as i64,
        // start_index: q.start_index.unwrap_or_else(|| 0),
        ..Default::default()
    }))
}

/// This sbould hold dynamic collections
#[get("/userviews")]
pub async fn userviews(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    let library_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection, db::MediaKind::Folder]),
        promoted: Some(true),
        ..Default::default()
    };
    let channel_filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::TvChannel]),
        enabled: Some(true),
        ..Default::default()
    };
    let (library_result, channel_result) = tokio::join!(
        db::Media::get_by_filter(&state.ctx.db, &library_filter),
        db::Media::get_by_filter(&state.ctx.db, &channel_filter),
    );

    let mut items = library_result?
        .records
        .into_iter()
        .map(jellyfin::db_media_to_item)
        .collect::<Vec<jellyfin::BaseItemDto>>();

    // Inject a synthetic Live TV view if any enabled channels exist
    if !channel_result?.records.is_empty() {
        items.push(livetv_view_item());
    }

    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        items,
        ..Default::default()
    }))
}

#[get("/userviews/groupingoptions")]
pub async fn userviews_groupingoptions(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    // Ok(Json(json!(
    // )))
    Ok(StatusCode::NO_CONTENT.into_response())
    // Ok(Json(json!(
    //     crate::jellyfin::get_virtual_folders(&state).await?
    // )))
}

#[get("/shows/nextup")]
pub async fn shows_nextup(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}
