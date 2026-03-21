use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use axum_anyhow::ApiResult as Result;
use remux_macros::get;

use crate::AppState;
use crate::jellyfin;

#[get("/artists")]
pub async fn get_artists(State(_state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        ..Default::default()
    }))
}

#[get("/artists/albumartists")]
pub async fn get_album_artists(State(_state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult {
        ..Default::default()
    }))
}
