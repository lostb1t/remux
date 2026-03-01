use anyhow::anyhow;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use axum_extra::extract::Query;
use chrono::{Duration, Local, Utc};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use headers;
use http::Response;
use http::StatusCode;
use remux_macros::{delete, get, post};
use serde::Deserialize;
use serde_json::json;
use std::io;
use tokio_util::io::ReaderStream;
use tracing::{info, trace};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use crate::jellyfin::MediaSourceInfoExt;
use crate::playback_session::PlaybackSession;
use crate::sdks;
use crate::utils;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

use super::stub;

#[post("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    let query_params = payload.clone();
    let media_source_id = payload.media_source_id;
    let device_profile = payload.device_profile;
    items_playbackinfo_inner(state, id, media_source_id, device_profile, query_params)
        .await
}

#[get("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo_get(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    let query_params = q.clone();
    let media_source_id = q.media_source_id;
    let device_profile = q.device_profile;
    items_playbackinfo_inner(state, id, media_source_id, device_profile, query_params)
        .await
}

async fn items_playbackinfo_inner(
    state: AppState,
    id: Uuid,
    media_source_id: Option<Uuid>,
    device_profile: Option<jellyfin::DeviceProfile>,
    query_params: jellyfin::PlaybackInfoQuery,
) -> Result<impl IntoResponse> {
    trace!(?id, ?media_source_id, "items_playbackinfo");

    let media = db::Media::get_by_id(&state.ctx.db, &media_source_id.unwrap_or(id))
        .await?
        .context_not_found("not found", "not found")?;

    let mut source = jellyfin::media_source_from_db(media);
    source.probe_in_place()?;

    // Determine if transcoding is needed based on device profile and query parameters
    let needs_transcoding = device_profile
        .as_ref()
        .map(|profile| {
            // Check if transcoding is explicitly enabled/disabled
            let transcoding_enabled = query_params.enable_transcoding.unwrap_or(false);

            // Check if direct play is enabled/disabled
            let direct_play_enabled = query_params.enable_direct_play.unwrap_or(true);
            let direct_play_supported = profile.supports_direct_play(&source);

            transcoding_enabled || !direct_play_supported || !direct_play_enabled
        })
        .unwrap_or(true); // Default to transcoding if no device profile

    tracing::debug!(
        "playback decision - needs transcoding: {}",
        needs_transcoding
    );

    let play_session_id = utils::get_uuid().as_simple().to_string();

    if needs_transcoding {
        // Pick container/protocol from the client's device profile, falling back to ts/hls
        let (trans_container, trans_protocol) = device_profile
            .as_ref()
            .and_then(|p| p.video_transcoding_profile())
            .map(|p| {
                (
                    p.container.clone().unwrap_or_else(|| "ts".to_string()),
                    p.protocol.clone().unwrap_or_else(|| "hls".to_string()),
                )
            })
            .unwrap_or_else(|| ("ts".to_string(), "hls".to_string()));

        source.supports_transcoding = Some(true);
        source.transcoding_url = Some(format!(
            "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec=h264&AudioCodec=aac",
            id, play_session_id, source.id
        ));
        source.transcoding_container = Some(trans_container);
        source.transcoding_sub_protocol = Some(trans_protocol);
        source.supports_direct_play = Some(false);
        source.supports_direct_stream = Some(false);
    } else {
        // Direct play - no transcoding needed
        source.supports_transcoding = Some(false);
        source.supports_direct_play = Some(true);
    }

    let info = jellyfin::PlaybackInfoResponse {
        media_sources: vec![source],
        play_session_id: Some(play_session_id),
        ..Default::default()
    };

    trace!(?info, "items_playbackinfo_result");
    Ok(Json(info))
}

/// Starts a direct playback for the given media UUID.
///
/// # Range
///
/// The `Range` header is forwarded to the upstream server. If no `Range` is provided,
/// the full video is sent.
///
/// # Static
///
/// If the `static_` query parameter is set to `true`, the response will be a static
/// video stream. Otherwise, a progressive transcode is started.
#[get("/videos/{id}/stream")]
pub async fn videos_stream(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    videos_stream_inner(headers, state, id, q).await
}

#[get("/videos/{id}/stream.{container}")]
pub async fn videos_stream_by_container(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path((id, container)): Path<(Uuid, String)>,
    Query(mut q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    if q.container.is_none() {
        q.container = Some(container);
    }
    videos_stream_inner(headers, state, id, q).await
}

async fn videos_stream_inner(
    headers: headers::HeaderMap,
    state: AppState,
    id: Uuid,
    q: jellyfin::VideoStreamQuery,
) -> Result<impl IntoResponse> {
    let mut media =
        db::Media::get_by_id(&state.ctx.db, &q.media_source_id.unwrap_or(id))
            .await?
            .context_not_found("not found", "not found")?;

    if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
        media = media
            .sources(&state.ctx.db)
            .await?
            .get(0)
            .context_not_found("not found", "not found")?
            .clone();
    }

    let url = media
        .url
        .clone()
        .context_not_found("no url", "media source has no URL")?;

    // Direct play: proxy the original stream with range support
    if q.static_.unwrap_or(false) || q.video_codec.is_none() {
        info!("starting direct playback for: {:?}", &media.title);
        let mut req = reqwest::Client::new().get(&url);
        if let Some(v) = headers.get(http::header::RANGE) {
            req = req.header(http::header::RANGE, v.clone());
        }

        let upstream = req.send().await?;

        let status = upstream.status();
        let headers_in = upstream.headers().clone();
        let upstream_stream = upstream.bytes_stream();
        let body = Body::from_stream(upstream_stream.map_err(io::Error::other));

        trace!(?status, ?headers_in, "videos_stream");

        let mut resp_out = axum::response::Response::builder()
            .status(status)
            .body(body)
            .unwrap();

        {
            use axum::http::header;
            let out_headers = resp_out.headers_mut();
            for (k, v) in headers_in.iter() {
                match k.as_str().to_ascii_lowercase().as_str() {
                    "content-length" | "content-type" | "accept-ranges"
                    | "content-range" | "last-modified" => {}
                    _ => continue,
                }
                out_headers.insert(k, v.clone());
            }

            if !out_headers.contains_key(header::CONTENT_TYPE) {
                out_headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("video/mp4"),
                );
            }
        }

        return Ok(resp_out);
    }

    // Progressive transcode: pipe ffmpeg output directly to response
    let container = q.container.as_deref().unwrap_or("mp4").to_string();
    let video_codec = q.video_codec.unwrap_or_else(|| "copy".to_string());
    let audio_codec = q.audio_codec.unwrap_or_else(|| "aac".to_string());

    info!(
        "starting progressive transcode for: {:?} (container={}, vcodec={}, acodec={})",
        &media.title, container, video_codec, audio_codec
    );

    let params = crate::transcode::engine::ProgressiveTranscodeParams {
        input_url: url,
        container: container.clone(),
        video_codec,
        audio_codec,
        start_time_ticks: q.start_time_ticks,
        max_width: q.max_width.map(|v| v as u32),
        max_height: q.max_height.map(|v| v as u32),
        video_bitrate: q.video_bit_rate.map(|v| v as u32),
        audio_bitrate: q.audio_bit_rate.map(|v| v as u32),
        audio_channels: q.audio_channels.map(|v| v as u32),
        audio_stream_index: q.audio_stream_index.map(|v| v as i32),
        subtitle_stream_index: q.subtitle_stream_index.map(|v| v as i32),
    };

    let stdout = crate::transcode::engine::start_progressive_transcode(params)?;
    let stream = ReaderStream::new(stdout);
    let body = Body::from_stream(stream);

    let content_type = match container.as_str() {
        "ts" | "mpegts" => "video/mp2t",
        "webm" => "video/webm",
        "mkv" | "matroska" => "video/x-matroska",
        _ => "video/mp4",
    };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "no-cache, no-store")
        .body(body)
        .unwrap())
}

#[post("/sessions/playing")]
pub async fn report_playback_start(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackStartInfo>,
) -> Result<impl IntoResponse> {
    let play_session_id = data
        .play_session_id
        .clone()
        .unwrap_or_else(|| utils::get_uuid().as_simple().to_string());

    let item_id = data.item_id.unwrap_or_default();

    let ps = PlaybackSession {
        play_session_id: play_session_id.clone(),
        user_id: session.user.id,
        item_id,
        media_source_id: data.media_source_id.clone(),
        device_id: session.device.id.clone(),
        client_name: session.device.app_name.clone(),
        position_ticks: data.position_ticks.unwrap_or(0),
        is_paused: data.is_paused,
        is_muted: data.is_muted,
        volume_level: data.volume_level,
        audio_stream_index: data.audio_stream_index,
        subtitle_stream_index: data.subtitle_stream_index,
        play_method: data.play_method.as_ref().map(|m| m.to_string()),
        started_at: Utc::now(),
        last_activity: Utc::now(),
    };

    ps.save(&state.ctx.store);
    info!(play_session_id, %item_id, "Playback started");

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use http::header::HeaderValue;
    use serde_json::json;

    const AUTH_HEADER: &str = "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\"";

    async fn authenticated_server() -> (axum_test::TestServer, String) {
        let server = crate::integration_test::new_test_server().await.unwrap();

        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER),
            )
            .json(&json!({
                "Username": "test",
                "Pw": "test"
            }))
            .await;

        let body: serde_json::Value = resp.json();
        let token = body["AccessToken"].as_str().unwrap().to_string();
        (server, token)
    }

    fn auth_header_with_token(token: &str) -> String {
        format!(
            "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\", Token=\"{}\"",
            token
        )
    }

    #[tokio::test]
    async fn test_playback_start() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "VolumeLevel": 100,
                "IsMuted": false,
                "IsPaused": false,
                "RepeatMode": "RepeatNone",
                "MaxStreamingBitrate": 3000000,
                "PositionTicks": 0,
                "PlayMethod": "DirectPlay",
                "PlaySessionId": "test-session-001",
                "MediaSourceId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "CanSeek": true,
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "NowPlayingQueue": [
                    {
                        "Id": "80ce1832bb797ffafaf65059b8b3dc9e",
                        "PlaylistItemId": "playlistItem0"
                    }
                ]
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_start_minimal_payload() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Clients may send very minimal payloads
        let resp = server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-minimal"
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_progress() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Start playback first
        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-progress",
                "PositionTicks": 0
            }))
            .await;

        // Report progress
        let resp = server
            .post("/sessions/playing/progress")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-progress",
                "PositionTicks": 300000000,
                "IsPaused": false,
                "IsMuted": false,
                "VolumeLevel": 80,
                "AudioStreamIndex": 1,
                "SubtitleStreamIndex": 0
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_stopped() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Start playback
        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-stop",
                "PositionTicks": 0
            }))
            .await;

        // Stop playback
        let resp = server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-stop",
                "PositionTicks": 500000000
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_full_lifecycle() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let psid = "test-session-lifecycle";

        // 1. Start
        let resp = server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": psid,
                "PositionTicks": 0,
                "CanSeek": true,
                "PlayMethod": "DirectPlay"
            }))
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // 2. Progress updates
        for ticks in [100_000_000i64, 200_000_000, 500_000_000] {
            let resp = server
                .post("/sessions/playing/progress")
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_str(&auth).unwrap(),
                )
                .json(&json!({
                    "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                    "PlaySessionId": psid,
                    "PositionTicks": ticks,
                    "IsPaused": false,
                    "IsMuted": false
                }))
                .await;
            resp.assert_status(StatusCode::NO_CONTENT);
        }

        // 3. Ping
        let resp = server
            .post(&format!("/sessions/playing/ping?PlaySessionId={}", psid))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // 4. Stop
        let resp = server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": psid,
                "PositionTicks": 600_000_000i64
            }))
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_ping_session() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .post("/sessions/playing/ping?PlaySessionId=some-session-id")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_progress_without_start_is_noop() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Progress with non-existent session should still return 204
        let resp = server
            .post("/sessions/playing/progress")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "nonexistent-session",
                "PositionTicks": 100000000
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_playback_stopped_without_start_is_noop() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "nonexistent-session",
                "PositionTicks": 100000000
            }))
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_get_sessions_empty() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .get("/sessions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let sessions: Vec<crate::jellyfin::SessionInfoDto> = resp.json();
        // One device session exists from the authentication step
        assert_eq!(sessions.len(), 1);
    }

    #[tokio::test]
    async fn test_get_sessions_with_active_session() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Start a playback session
        let psid = "test-session-get";
        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": psid,
                "PositionTicks": 0
            }))
            .await;

        // Get all sessions
        let resp = server
            .get("/sessions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let sessions: Vec<crate::jellyfin::SessionInfoDto> = resp.json();
        assert_eq!(sessions.len(), 1);
        // id is the device id from the auth header, not the play session id
        assert_eq!(sessions[0].id, Some("test-device".to_string()));
        // now_playing_item is populated for the active playback session
        assert!(sessions[0].now_playing_item.is_some());
    }
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        if let Some(mut ps) = PlaybackSession::get(&state.ctx.store, psid) {
            ps.position_ticks = data.position_ticks.unwrap_or(ps.position_ticks);
            ps.is_paused = data.is_paused;
            ps.is_muted = data.is_muted;
            ps.volume_level = data.volume_level.or(ps.volume_level);
            ps.audio_stream_index = data.audio_stream_index.or(ps.audio_stream_index);
            ps.subtitle_stream_index =
                data.subtitle_stream_index.or(ps.subtitle_stream_index);
            ps.last_activity = Utc::now();
            ps.save(&state.ctx.store);

            // persist position to db
            let item_id = data.item_id.unwrap_or(ps.item_id);
            if let Ok(Some(media)) = db::Media::get_by_id(&state.ctx.db, &item_id).await
            {
                let position_seconds = ps.position_ticks / 10_000_000;
                let mut ms = db::UserMediaState::get_or_new(
                    &state.ctx.db,
                    &session.user,
                    &media,
                )
                .await?;
                ms.playback_position = position_seconds;
                ms.audio_idx = ps.audio_stream_index.map(|x| x as i64);
                ms.subtitle_idx = ps.subtitle_stream_index.map(|x| x as i64);
                ms.save(&state.ctx.db).await?;
            }
        }
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/playing/stopped")]
pub async fn report_playback_stopped(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackStopInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        let ps = PlaybackSession::remove(&state.ctx.store, psid);

        let item_id = data.item_id.or(ps.as_ref().map(|s| s.item_id));
        let final_ticks = data
            .position_ticks
            .or(ps.as_ref().map(|s| s.position_ticks));

        if let Some(item_id) = item_id {
            if let Ok(Some(media)) = db::Media::get_by_id(&state.ctx.db, &item_id).await
            {
                let position_seconds = final_ticks.unwrap_or(0) / 10_000_000;
                let mut ms = db::UserMediaState::get_or_new(
                    &state.ctx.db,
                    &session.user,
                    &media,
                )
                .await?;
                ms.playback_position = position_seconds;
                // If watched to near the end (>= 90%), mark as played
                if let Some(runtime) = media.runtime {
                    let runtime_seconds = runtime;
                    if runtime_seconds > 0
                        && position_seconds >= (runtime_seconds * 90 / 100)
                    {
                        ms.play_count += 1;
                        ms.played_at = Some(Local::now().naive_local());
                        ms.playback_position = 0;
                    }
                }
                ms.save(&state.ctx.db).await?;
            }
        }

        info!(play_session_id = psid, "Playback stopped");
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[derive(Deserialize)]
pub struct PingQuery {
    #[serde(alias = "playSessionId", alias = "PlaySessionId")]
    pub play_session_id: String,
}

#[post("/sessions/playing/ping")]
pub async fn ping_playback_session(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<PingQuery>,
) -> Result<impl IntoResponse> {
    PlaybackSession::ping(&state.ctx.store, &q.play_session_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/capabilities/full")]
pub async fn sessions_capabilities_full(
    State(state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    stub(State(state)).await
}

#[derive(Deserialize, Default)]
struct SessionsQuery {
    #[serde(rename = "activeWithinSeconds", alias = "ActiveWithinSeconds")]
    active_within_seconds: Option<i64>,
}

/// Get all active sessions
#[get("/sessions")]
pub async fn get_sessions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<SessionsQuery>,
) -> Result<impl IntoResponse> {
    let cutoff = q
        .active_within_seconds
        .map(|s| Utc::now() - Duration::seconds(s));
    let devices = auth::Device::get_all(&state.ctx.db).await?;
    let playback_sessions = PlaybackSession::get_all(&state.ctx.store);

    let sessions = devices
        .into_iter()
        .filter(|device| {
            if let Some(cutoff) = cutoff {
                device.last_activity_at.map_or(true, |t| t >= cutoff)
            } else {
                true
            }
        })
        .map(|device| {
            // Attach now-playing info if this device has an active playback session.
            let now_playing = playback_sessions
                .iter()
                .find(|s| s.device_id == device.id)
                .and_then(|s| {
                    // Minimal stub so clients know something is playing.
                    Some(jellyfin::BaseItemDto {
                        id: s.item_id,
                        ..Default::default()
                    })
                });

            jellyfin::SessionInfoDto {
                id: Some(device.id.clone()),
                user_id: device.user_id.to_string(),
                user_name: None,
                client: Some(device.app_name.clone()),
                last_activity_date: device.last_activity_at.unwrap_or_else(Utc::now),
                last_playback_check_in: device
                    .last_activity_at
                    .unwrap_or_else(Utc::now),
                last_paused_date: None,
                device_name: Some(device.name.clone()),
                device_type: None,
                now_playing_item: now_playing,
                now_viewing_item: None,
                device_id: Some(device.id.clone()),
                application_version: Some(device.app_version.clone()),
                is_active: true,
                supports_media_control: false,
                supports_remote_control: false,
                has_custom_device_name: false,
                playlist_item_id: None,
                server_id: Some(crate::utils::server_id()),
                user_primary_image_tag: None,
                playable_media_types: vec![],
                remote_end_point: None,
                now_playing_queue: None,
                now_playing_queue_full_items: None,
            }
        })
        .collect::<Vec<jellyfin::SessionInfoDto>>();

    Ok(Json(sessions))
}

#[post("/userplayeditems/{id}")]
pub async fn user_mark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("not found", "not found")?;
    let ms = media.mark_played(&state.ctx.db, &session.user).await?;
    Ok(Json(jellyfin::db_state_to_dto(ms)).into_response())
}

#[delete("/userplayeditems/{id}")]
pub async fn user_unmark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("not found", "not found")?;
    let ms = media.mark_unplayed(&state.ctx.db, &session.user).await?;
    Ok(Json(jellyfin::db_state_to_dto(ms)).into_response())
}

/// Jellyfin-compatible master HLS playlist endpoint.
/// Creates a transcode session and returns a master.m3u8 playlist.
#[get("/videos/{id}/master.m3u8")]
pub async fn master_hls_video(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    info!("master_hls_video: item_id={}, q={:?}", id, q);

    // Add debugging info for crash diagnosis
    tracing::debug!(
        "Starting HLS session setup for item {} with session ID: {:?}",
        id,
        q.play_session_id
    );

    let play_session_id = q
        .play_session_id
        .unwrap_or_else(|| utils::get_uuid().as_simple().to_string());

    tracing::debug!("Using play session ID: {}", play_session_id);

    let video_codec = q.video_codec.unwrap_or_else(|| "copy".to_string());
    let audio_codec = q.audio_codec.unwrap_or_else(|| "aac".to_string());
    let segment_length = q.segment_length.unwrap_or(6) as u32;

    // Look up existing session or create a new one
    let session = if let Some(existing) = state.ctx.transcode.get(&play_session_id) {
        existing
    } else {
        // Fetch media info to get the stream URL
        let media_source_id = q.media_source_id.unwrap_or(id);
        let media = db::Media::get_by_id(&state.ctx.db, &media_source_id)
            .await?
            .context_not_found("not found", "media not found")?;

        let mut resolved_media = media.clone();
        if resolved_media.kind == db::MediaKind::Movie
            || resolved_media.kind == db::MediaKind::Episode
        {
            resolved_media = resolved_media
                .sources(&state.ctx.db)
                .await?
                .get(0)
                .context_not_found("not found", "source not found")?
                .clone();
        }

        let input_url = resolved_media
            .url
            .context_not_found("no url", "media source has no URL")?;

        let session = state.ctx.transcode.create(
            play_session_id.clone(),
            id,
            media_source_id,
            input_url.clone(),
            video_codec.clone(),
            audio_codec.clone(),
            segment_length,
        );

        // Start transcoding in background
        let session_clone = session.clone();
        let params = crate::transcode::engine::TranscodeParams {
            input_url,
            output_dir: session.read().await.output_dir.clone(),
            video_codec: video_codec.clone(),
            audio_codec: audio_codec.clone(),
            segment_length,
            start_time_ticks: q.start_time_ticks,
            max_width: q.max_width.map(|v| v as u32),
            max_height: q.max_height.map(|v| v as u32),
            video_bitrate: q.video_bit_rate.map(|v| v as u32),
            audio_bitrate: q.audio_bit_rate.map(|v| v as u32),
            audio_channels: None,
            audio_stream_index: q.audio_stream_index.map(|v| v as i32),
            subtitle_stream_index: q.subtitle_stream_index.map(|v| v as i32),
        };

        // Spawn the transcode task with proper error handling
        let session_clone = session.clone();
        tokio::spawn(async move {
            if let Err(e) =
                crate::transcode::engine::start_transcode(session_clone, params).await
            {
                tracing::error!("Transcode failed: {:#}", e);
            }
        });

        session
    };

    // Generate and return the master playlist
    let session_read = session.read().await;
    let master_playlist =
        crate::transcode::engine::generate_master_playlist(&session_read);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/vnd.apple.mpegurl")
        .header("Cache-Control", "no-cache, no-store")
        .body(Body::from(master_playlist))
        .unwrap())
}

/// Variant HLS playlist - alternate URL used by some clients.
#[get("/videos/{id}/main.m3u8")]
pub async fn variant_hls_video_alt(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    variant_hls_video_inner(state, q).await
}

/// Serves the variant (child) HLS playlist generated by the transcoding engine.
#[get("/videos/{id}/main/stream.m3u8")]
pub async fn variant_hls_video(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    variant_hls_video_inner(state, q).await
}

async fn variant_hls_video_inner(
    state: AppState,
    q: jellyfin::HlsVideoQuery,
) -> Result<impl IntoResponse> {
    let play_session_id = q
        .play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    let session = state
        .ctx
        .transcode
        .get(&play_session_id)
        .context_not_found("not found", "transcode session not found")?;

    let playlist_path = session.read().await.variant_playlist_path();

    // Wait up to 30 seconds for the playlist to be created
    let mut attempts = 0;
    while !playlist_path.exists() && attempts < 60 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        attempts += 1;
    }

    if !playlist_path.exists() {
        return Err(anyhow!("Variant playlist not ready after timeout").into());
    }

    // Update last accessed
    session.write().await.last_accessed = std::time::Instant::now();

    let content = tokio::fs::read_to_string(&playlist_path).await?;

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/vnd.apple.mpegurl")
        .header("Cache-Control", "no-cache, no-store")
        .body(Body::from(content))
        .unwrap())
}

/// Serves individual HLS segment files.
/// Captures the full segment filename (e.g. "segment_00001.ts") and strips the extension.
#[get("/videos/{id}/main/{segment_file}")]
pub async fn hls_segment(
    State(state): State<AppState>,
    Path((id, segment_file)): Path<(Uuid, String)>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let segment_id = strip_segment_extension(&segment_file);
    hls_segment_inner(state, segment_id, q).await
}

/// Jellyfin-compatible HLS segment route: /Videos/{id}/hls1/{playlistId}/{segmentFile}
#[get("/videos/{id}/hls1/{playlist_id}/{segment_file}")]
pub async fn hls1_segment(
    State(state): State<AppState>,
    Path((id, _playlist_id, segment_file)): Path<(Uuid, String, String)>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let segment_id = strip_segment_extension(&segment_file);
    hls_segment_inner(state, segment_id, q).await
}

fn strip_segment_extension(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(name, _ext)| name.to_string())
        .unwrap_or_else(|| filename.to_string())
}

async fn hls_segment_inner(
    state: AppState,
    segment_id: String,
    q: jellyfin::HlsVideoQuery,
) -> Result<impl IntoResponse> {
    let play_session_id = q
        .play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    let session = state
        .ctx
        .transcode
        .get(&play_session_id)
        .context_not_found("not found", "transcode session not found")?;

    let segment_path = session.read().await.segment_path(&segment_id);

    // Wait for the segment to be written (up to 60 seconds)
    let mut attempts = 0;
    while !segment_path.exists() && attempts < 120 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        attempts += 1;
    }

    if !segment_path.exists() {
        return Err(anyhow!("Segment {} not ready after timeout", segment_id).into());
    }

    // Update last accessed
    session.write().await.last_accessed = std::time::Instant::now();

    let file = tokio::fs::File::open(&segment_path).await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "video/mp2t")
        .header("Cache-Control", "public, max-age=86400")
        .body(body)
        .unwrap())
}

/// Stops and cleans up a transcoding session.
#[delete("/videos/activeencodings")]
pub async fn delete_transcoding(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    if let Some(play_session_id) = q.play_session_id {
        info!("Stopping transcode session: {}", play_session_id);
        state.ctx.transcode.stop(&play_session_id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

/// Returns additional parts for a multi-file video item.
#[get("/videos/{id}/additionalparts")]
pub async fn video_additional_parts(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::BaseItemDtoQueryResult::default()))
}

/// Bitrate test endpoint - returns a body of the requested size for bandwidth measurement.
#[get("/playback/bitratetest")]
pub async fn playback_bitratetest_sized(
    Query(q): Query<BitrateTestQuery>,
) -> Result<impl IntoResponse> {
    let size = q.size.unwrap_or(100_000).min(10_000_000) as usize;
    let body = vec![0u8; size];
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", size.to_string())
        .body(Body::from(body))
        .unwrap())
}

#[derive(Deserialize)]
pub struct BitrateTestQuery {
    #[serde(alias = "Size", alias = "size")]
    pub size: Option<u64>,
}
