use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query as ExtraQuery;
use remux_macros::get;

use crate::api::items::get_items;
use crate::db::auth;
use crate::{AppState, api};

/// `/Artists` — returns all artists in the library.
///
/// Jellyfin music clients call this to populate the Artists view.
/// We delegate to `get_items` with `IncludeItemTypes=MusicArtist`.
#[get("/artists")]
pub async fn get_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(mut q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.include_item_types = Some(vec![api::MediaType::MusicArtist]);
    q.recursive = true;
    let result = get_items(state, session, q, true).await?;
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result.items,
        total_record_count: result.total_count,
        start_index: 0,
    }))
}

/// `/Artists/AlbumArtists` — same as `/Artists` for our purposes.
#[get("/artists/albumartists")]
pub async fn get_album_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(mut q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    q.include_item_types = Some(vec![api::MediaType::MusicArtist]);
    q.recursive = true;
    let result = get_items(state, session, q, true).await?;
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result.items,
        total_record_count: result.total_count,
        start_index: 0,
    }))
}
