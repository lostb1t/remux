use axum::Json;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum_anyhow::ApiResult as Result;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::providers::LyricSearchRequest;

/// `GET /Audio/{item_id}/Lyrics` — fetch the best lyric match for a track.
#[get("/audio/{item_id}/lyrics")]
pub async fn get_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(item_id): Path<Uuid>,
) -> Result<Response> {
    let Some(media) = db::Media::get_by_id(&state.ctx.db, &item_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if media.kind != db::MediaKind::Track {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let req = build_search_request(&state.ctx.db, &media).await;

    let Some(lyrics) = state.ctx.lyrics.fetch(&req).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };

    Ok(Json(lyrics).into_response())
}

/// `GET /Audio/{item_id}/RemoteSearch/Lyrics` — search all providers for lyrics candidates.
#[get("/audio/{item_id}/remotesearch/lyrics")]
pub async fn search_remote_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(item_id): Path<Uuid>,
) -> Result<Response> {
    let Some(media) = db::Media::get_by_id(&state.ctx.db, &item_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    if media.kind != db::MediaKind::Track {
        return Ok(StatusCode::NOT_FOUND.into_response());
    }

    let req = build_search_request(&state.ctx.db, &media).await;
    let results = state.ctx.lyrics.search(&req).await?;

    Ok(Json(results).into_response())
}

/// `GET /Providers/Lyrics/{lyric_id}` — fetch a specific lyric by composite ID (e.g. `lrclib_3396226`).
#[get("/providers/lyrics/{lyric_id}")]
pub async fn get_provider_lyrics(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(lyric_id): Path<String>,
) -> Result<Response> {
    let Some(lyrics) = state.ctx.lyrics.get_by_composite_id(&lyric_id).await? else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(Json(lyrics).into_response())
}

async fn build_search_request(db: &sqlx::SqlitePool, media: &db::Media) -> LyricSearchRequest {
    let (artist, album) = resolve_music_titles(db, media).await;
    LyricSearchRequest {
        title: media.title.clone(),
        artist,
        album,
        duration_secs: media.runtime.map(|r| r as f64),
    }
}

pub(crate) async fn resolve_music_titles(
    db: &sqlx::SqlitePool,
    media: &db::Media,
) -> (Option<String>, Option<String>) {
    let ids: Vec<Uuid> = [media.series_id, media.parent_id]
        .into_iter()
        .flatten()
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    if ids.is_empty() {
        return (None, None);
    }

    let mut qb = sqlx::QueryBuilder::new("SELECT id, title FROM media WHERE id IN (");
    let mut sep = qb.separated(", ");
    for id in &ids {
        sep.push_bind(id);
    }
    qb.push(")");

    let map: std::collections::HashMap<Uuid, String> = qb
        .build()
        .fetch_all(db)
        .await
        .unwrap_or_default()
        .into_iter()
        .filter_map(|r| {
            use sqlx::Row;
            let id: Option<Uuid> = r.get(0);
            let title: Option<String> = r.get(1);
            id.zip(title)
        })
        .collect();

    let artist = media.series_id.and_then(|id| map.get(&id).cloned());
    let album = media.parent_id.and_then(|id| map.get(&id).cloned());
    (artist, album)
}
