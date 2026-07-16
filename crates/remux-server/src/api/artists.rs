use axum::{
    Json,
    extract::{Path, Query, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query as ExtraQuery;
use remux_macros::get;
use uuid::Uuid;

use crate::{AppState, OptionExt, api, api::items::get_items, db, db::auth};

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

/// `/Artists/{name}` — returns a single artist item by display name.
#[get("/artists/{name}")]
pub async fn get_artist_by_name(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(name): Path<String>,
) -> Result<impl IntoResponse> {
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM media WHERE kind = 'artist' AND LOWER(title) = LOWER(?) LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(&state.ctx.db)
    .await?
    .context_not_found("Artist not found")?;
    let artist = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("Artist not found")?;
    Ok(Json(api::db_media_to_item(artist, false)))
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
