use axum::Json;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_anyhow::{ApiResult as Result, OptionExt};
use axum_extra::extract::Query;
use remux_macros::{api_query, get};
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db;
use crate::db::auth::AuthSession;

#[api_query]
#[derive(Debug)]
pub struct InstantMixQuery {
    pub user_id: Option<Uuid>,
    pub limit: Option<u32>,
}

// ---------------------------------------------------------------------------
// Core helper
// ---------------------------------------------------------------------------

async fn genre_ids_for(db: &sqlx::SqlitePool, media_id: Uuid) -> Vec<Uuid> {
    sqlx::query_scalar::<_, Uuid>(
        "SELECT mr.right_media_id FROM media_relations mr \
         JOIN media g ON g.id = mr.right_media_id \
         WHERE mr.left_media_id = ? AND g.kind = 'genre'",
    )
    .bind(media_id)
    .fetch_all(db)
    .await
    .unwrap_or_default()
}

async fn build_mix(
    ctx: &crate::AppContext,
    session: &AuthSession,
    genre_ids: Vec<Uuid>,
    artist_ids: Vec<Uuid>,
    limit: Option<u32>,
) -> Result<Vec<db::Media>> {
    use remux_sdks::remux::ItemSortBy;

    let (use_genre_ids, use_artist_ids) = if !genre_ids.is_empty() {
        (genre_ids, vec![])
    } else {
        (vec![], artist_ids)
    };

    let filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Track]),
        genre_ids: if use_genre_ids.is_empty() {
            None
        } else {
            Some(use_genre_ids)
        },
        artist_ids: if use_artist_ids.is_empty() {
            None
        } else {
            Some(use_artist_ids)
        },
        sort_by: vec![ItemSortBy::Random],
        limit: Some(limit.unwrap_or(50)),
        include_user_state: true,
        user_id: Some(session.user.id),
        total_count: false,
        ..Default::default()
    };

    let result = db::Media::get_by_filter(&ctx.db, &filter).await?;
    Ok(result.records)
}

fn mix_response(items: Vec<db::Media>) -> impl IntoResponse {
    let total = items.len() as i64;
    let dtos: Vec<api::BaseItemDto> =
        items.into_iter().map(api::db_media_to_item).collect();
    Json(api::BaseItemDtoQueryResult {
        items: dtos,
        total_record_count: total,
        start_index: 0,
        ..Default::default()
    })
}

// ---------------------------------------------------------------------------
// GET /Songs/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/songs/{item_id}/instantmix")]
pub async fn instant_mix_song(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let track = db::Media::get_by_id(&state.ctx.db, &item_id)
        .await?
        .context_not_found("Not Found", "Song not found")?;

    let genre_ids = genre_ids_for(&state.ctx.db, track.id).await;
    let artist_ids = track.grandparent_id.into_iter().collect();

    let items = build_mix(&state.ctx, &session, genre_ids, artist_ids, q.limit).await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Albums/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/albums/{item_id}/instantmix")]
pub async fn instant_mix_album(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let album = db::Media::get_by_id(&state.ctx.db, &item_id)
        .await?
        .context_not_found("Not Found", "Album not found")?;

    let genre_ids = genre_ids_for(&state.ctx.db, album.id).await;
    let artist_ids = album.parent_id.into_iter().collect();

    let items = build_mix(&state.ctx, &session, genre_ids, artist_ids, q.limit).await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Artists/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/artists/{item_id}/instantmix")]
pub async fn instant_mix_artist(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &item_id)
        .await?
        .context_not_found("Not Found", "Artist not found")?;

    let items = build_mix(&state.ctx, &session, vec![], vec![item_id], q.limit).await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Playlists/{itemId}/InstantMix
// ---------------------------------------------------------------------------

#[get("/playlists/{item_id}/instantmix")]
pub async fn instant_mix_playlist(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    db::Media::get_by_id(&state.ctx.db, &item_id)
        .await?
        .context_not_found("Not Found", "Playlist not found")?;

    // Fetch tracks in the playlist to gather their genres and artists.
    let tracks = db::Media::get_by_filter(
        &state.ctx.db,
        &db::MediaFilter {
            kind: Some(vec![db::MediaKind::Track]),
            parent_id: Some(item_id),
            limit: Some(200),
            ..Default::default()
        },
    )
    .await?
    .records;

    let track_ids: Vec<Uuid> = tracks.iter().map(|t| t.id).collect();
    let artist_ids: Vec<Uuid> =
        tracks.iter().filter_map(|t| t.grandparent_id).collect();

    let genre_ids: Vec<Uuid> = if track_ids.is_empty() {
        vec![]
    } else {
        let mut ids = Vec::new();
        for tid in &track_ids {
            ids.extend(genre_ids_for(&state.ctx.db, *tid).await);
        }
        ids.sort_unstable();
        ids.dedup();
        ids
    };

    let items = build_mix(&state.ctx, &session, genre_ids, artist_ids, q.limit).await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /Items/{itemId}/InstantMix  (dispatch by kind)
// ---------------------------------------------------------------------------

#[get("/items/{item_id}/instantmix")]
pub async fn instant_mix_item(
    State(state): State<AppState>,
    session: AuthSession,
    Path(item_id): Path<Uuid>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &item_id)
        .await?
        .context_not_found("Not Found", "Item not found")?;

    let (genre_ids, artist_ids) = match media.kind {
        db::MediaKind::Track => {
            let g = genre_ids_for(&state.ctx.db, media.id).await;
            let a = media.grandparent_id.into_iter().collect();
            (g, a)
        }
        db::MediaKind::Album => {
            let g = genre_ids_for(&state.ctx.db, media.id).await;
            let a = media.parent_id.into_iter().collect();
            (g, a)
        }
        db::MediaKind::Artist => (vec![], vec![media.id]),
        db::MediaKind::Genre => (vec![media.id], vec![]),
        _ => {
            let g = genre_ids_for(&state.ctx.db, media.id).await;
            (g, vec![])
        }
    };

    let items = build_mix(&state.ctx, &session, genre_ids, artist_ids, q.limit).await?;
    Ok(mix_response(items))
}

// ---------------------------------------------------------------------------
// GET /MusicGenres/{name}/InstantMix
// ---------------------------------------------------------------------------

#[get("/musicgenres/{name}/instantmix")]
pub async fn instant_mix_genre(
    State(state): State<AppState>,
    session: AuthSession,
    Path(name): Path<String>,
    Query(q): Query<InstantMixQuery>,
) -> Result<impl IntoResponse> {
    let genre = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM media WHERE kind = 'genre' AND LOWER(title) = LOWER(?) LIMIT 1",
    )
    .bind(&name)
    .fetch_optional(&state.ctx.db)
    .await?
    .context_not_found("Not Found", "Genre not found")?;

    let items = build_mix(&state.ctx, &session, vec![genre], vec![], q.limit).await?;
    Ok(mix_response(items))
}
