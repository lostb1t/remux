use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::get;
use axum_extra::extract::Query;
use http::StatusCode;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::{ApiResult as Result, OptionExt};

use super::mock_items;
use super::items::get_items;

#[get("/shows/{id}/seasons")]
pub async fn shows_seasons(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<jellyfin::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.parent_id = Some(id);
    q.include_item_types = Some(vec![jellyfin::MediaType::Season]);
    let items = get_items(state, session, q.clone(), true).await?;

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
    let items = get_items(state, session, q.clone(), true).await?;

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
    let manifest = session.aio.as_ref()
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?
        .get_manifest().await?;

    let mut items = db::Media::get_by_filter(
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
    .map(|x| {
        let mut item = jellyfin::db_media_to_item(x);
        //item.type_ = jellyfin::MediaType::CollectionFolder;
        //item.collection_type = Some(jellyfin::CollectionType::Movies);
        item
    })
    .collect::<Vec<jellyfin::BaseItemDto>>();

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
    let _manifest = session.aio.as_ref()
        .context_bad_request("AIO not configured", "Complete the setup wizard first")?
        .get_manifest().await?;

    // Ok(Json(json!(
    // )))
    Ok(StatusCode::NO_CONTENT.into_response())
    // Ok(Json(json!(
    //     crate::jellyfin::get_virtual_folders(&state).await?
    // )))
}

#[get("/shows/nextup")]
pub async fn shows_nextup(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}
