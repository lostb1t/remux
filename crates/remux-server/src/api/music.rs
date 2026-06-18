use axum::{Json, extract::State, response::IntoResponse};
use axum_extra::extract::Query;
use remux_macros::{api_query, get, post};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{AppState, api, db, db::auth};
use axum_anyhow::ApiResult as Result;

#[api_query]
#[derive(Debug)]
pub struct MusicSearchQuery {
    pub q: Option<String>,
    pub limit: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct MusicSearchResult {
    pub items: Vec<api::BaseItemDto>,
    pub total_record_count: i64,
}

/// Search for music tracks via yt-dlp.
///
/// `GET /music/search?q=<query>&limit=<n>`
#[get("/music/search")]
pub async fn music_search(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<MusicSearchQuery>,
) -> Result<impl IntoResponse> {
    let term =
        q.q.unwrap_or_default();
    let limit = q
        .limit
        .unwrap_or(20);

    if term.is_empty() {
        return Ok(Json(MusicSearchResult {
            items: vec![],
            total_record_count: 0,
        }));
    }

    let results = state
        .ctx
        .addons
        .search(&db::MediaKind::Track, &term, limit, &state.ctx)
        .await?;

    let items: Vec<api::BaseItemDto> = results
        .into_iter()
        .map(|m| api::models::db_media_to_item(m, false))
        .collect();

    let total = items.len() as i64;
    Ok(Json(MusicSearchResult {
        items,
        total_record_count: total,
    }))
}

#[derive(Debug, Deserialize)]
pub struct InsertTrackBody {
    /// YouTube video ID (e.g. "dQw4w9WgXcQ") or full URL.
    pub media_id: String,
    /// Override title; if omitted, yt-dlp fetches it.
    pub title: Option<String>,
}

/// Insert a music track into the library.
///
/// `POST /music/tracks`
///
/// Accepts a YouTube video ID or URL.  The track is persisted in the DB and
/// metadata is enriched via yt-dlp.
#[post("/music/tracks")]
pub async fn insert_track(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Json(body): Json<InsertTrackBody>,
) -> Result<impl IntoResponse> {
    let media_id = body
        .media_id
        .trim()
        .to_owned();

    // Normalise: if it looks like just an ID (no slashes), build a URL.
    let url = if media_id.starts_with("http://") || media_id.starts_with("https://") {
        media_id.clone()
    } else {
        format!("https://www.youtube.com/watch?v={}", media_id)
    };

    // Extract the bare video ID for storage (last path/query component).
    let video_id = if media_id.contains('/') || media_id.contains('?') {
        // parse ?v= from URL
        url.split("v=")
            .nth(1)
            .and_then(|s| {
                s.split('&')
                    .next()
            })
            .unwrap_or(&media_id)
            .to_owned()
    } else {
        media_id.clone()
    };

    let stable_id = crate::common::stable_media_uuid(&db::MediaKind::Track, &video_id);

    let mut media = db::Media {
        id: stable_id,
        title: body
            .title
            .unwrap_or_else(|| video_id.clone()),
        kind: db::MediaKind::Track,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::http(url.clone()),
            ..Default::default()
        }),
        external_ids: db::ExternalIds {
            youtube_id: Some(video_id.clone()),
            ..Default::default()
        },
        ..Default::default()
    };

    // Enrich with yt-dlp metadata (title, thumbnail, duration, description).
    let meta_config = crate::db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    if let Err(e) = state
        .ctx
        .addons
        .refresh_meta(&mut media, &state.ctx, true, &meta_config)
        .await
    {
        warn!(id = %media.id, error = %e, "yt-dlp metadata enrichment failed during track insert");
    }

    db::Media::upsert(
        &state
            .ctx
            .db,
        &[media.clone()],
    )
    .await?;

    Ok(Json(api::models::db_media_to_item(media, false)))
}
