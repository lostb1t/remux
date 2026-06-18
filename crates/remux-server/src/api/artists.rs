use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query as ExtraQuery;
use remux_macros::get;

use crate::{AppState, api, api::items::get_items, db::auth};

async fn artists_response(
    state: AppState,
    session: auth::AuthSession,
    mut q: api::GetItemsQuery,
) -> Result<impl IntoResponse> {
    q.include_item_types = Some(vec![api::MediaType::MusicArtist]);
    q.recursive = true;
    let result = get_items(state, session, q, true)
        .await?
        .with_client_patches()
        .build();
    Ok(Json(api::BaseItemDtoQueryResult {
        items: result.items,
        total_record_count: result.total_count,
        start_index: 0,
    }))
}

/// `/Artists` — returns all artists in the library.
#[get("/artists")]
pub async fn get_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    artists_response(state, session, q).await
}

/// `/Artists/AlbumArtists` — same as `/Artists` for our purposes.
#[get("/artists/albumartists")]
pub async fn get_album_artists(
    State(state): State<AppState>,
    session: auth::AuthSession,
    ExtraQuery(q): ExtraQuery<api::GetItemsQuery>,
) -> Result<impl IntoResponse> {
    artists_response(state, session, q).await
}
