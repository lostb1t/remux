use anyhow::anyhow;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::{delete, get, post};
use axum_extra::extract::Query;
use chrono::{Local, Utc};
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use headers;
use http::Response;
use http::StatusCode;
use serde::Deserialize;
use serde_json::json;
use std::io;
use tokio_util::io::ReaderStream;
use tracing::info;
use tracing::trace;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
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
    // Query(q): Query<jellyfin::PlaybackInfoQuery>,
    Json(payload): Json<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    trace!(?id, ?payload, "items_playbackinfo");

    //let mut item = session.item_store.get(&id);
    //let source = item.media_sources_mut(&session.aio)
    //        .await?
    //        .iter_mut()
    //        .find(|x| (q.media_source_id || x.id) == item.id)
    //        .expect("media source not found")
    //        .probe_in_place();

    //item.save(&session.item_store);
    let media =
        db::Media::get_by_id(&state.ctx.db, &payload.media_source_id.unwrap_or(id))
            .await?
            .context_not_found("not found", "not found")?;

    //let stream = id
    //    .stream
    //    .clone()
    //    .or_else(|| {
    //        payload
    //            .media_source_id
    //            .as_ref()
    //            .and_then(|m| m.stream.clone())
    //    })
    //    .context_not_found("not", "not")?;

    // let stream = session.aio.get_stream(
    //     id.jellyfin_media_type.into(),
    //     id.id.clone(),
    //     stream_id
    // ).await?;

    //let mut source: jellyfin::MediaSourceInfo =
    //    stream_into_media_source_info(id.id, id.jellyfin_media_type, stream);

    let mut source: jellyfin::MediaSourceInfo = media.into();
    source.probe_in_place()?;
    let subtitles: Vec<sdks::aio::Subtitle> = vec![];

    // info on tracks: https://github.com/jellyfin/Swiftfin/blob/main/Shared/Extensions/JellyfinAPI/MediaStream.swift#L219

    // Codec: "subrip" - Subtitle format codec (SRT)
    // TimeBase: "1/1000" - Time base in milliseconds
    // VideoRange: "Unknown" - Video color range (not applicable for subtitles)
    // VideoRangeType: "Unknown" - Color range type unknown
    // AudioSpatialFormat: "None" - No audio spatial format (subtitle)
    // LocalizedUndefined: "Undefined" - Label for undefined flag
    // LocalizedDefault: "Default" - Label for default flag
    // LocalizedForced: "Forced" - Label for forced subtitle flag
    // LocalizedExternal: "External" - Label for external subtitle source
    // LocalizedHearingImpaired: "Hearing Impaired" - Label for hearing impaired subtitles
    // DisplayTitle: "Undefined - SUBRIP - External" - Combined display title
    // IsInterlaced: false - Not interlaced (not applicable)
    // IsAVC: false - Not AVC video codec (subtitle)
    // IsDefault: false - Not default subtitle stream
    // IsForced: false - Not forced subtitle
    // IsHearingImpaired: false - Not hearing impaired subtitles
    // Height: 0 - Video height (not applicable)
    // Width: 0 - Video width (not applicable)
    // Type: "Subtitle" - Stream type is subtitle
    // Index: 0 - Stream index
    // IsExternal: true - Subtitle is external
    // DeliveryMethod: "External" - Delivered as external file
    // DeliveryUrl: "/Videos/657a70e0-ad75-82d8-2e64-c3a30c186a03/657a70e0ad7582d82e64c3a30c186a03/Subtitles/0/0/Stream.vtt?api_key=68068a69a1594bc1a1f34b394259630c" - URL for fetching subtitle stream
    // IsExternalUrl: false - DeliveryUrl is not an external URL
    // IsTextSubtitleStream: true - This is a text subtitle stream
    // SupportsExternalStream: true - External subtitle streams supported
    // Path: "/media/test/Ghosts.2021.S01E05.720p.AMZN.WEBRip.x264-GalaxyTV.srt" - Local file path for subtitle
    // Level: 0 - Subtitle level or priority

    let info = jellyfin::PlaybackInfoResponse {
        media_sources: vec![source],
        play_session_id: Some(utils::get_uuid().as_simple().to_string()),
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
/// video stream. Otherwise, a `jellyfin::PlaybackInfoResponse` is returned.
#[get("/videos/{id}/stream")]
pub async fn videos_stream(
    // range: Option<axum_extra::TypedHeader<headers::Range>>,
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    //session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    //let (id, media_type, stream_id) = utils::decode_media_token(&uuid)?;
    //  .log_err("Failed to decode media UUID")

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
    // trace!(?media, ?q, "videos_stream");

    // filter by id
    //let stream = id
    //    .stream
    //    .clone()
    //    .or_else(|| q.media_source_id.as_ref().and_then(|m| m.stream.clone()))
    //    .context_not_found("not", "not")?;

    //let stream = session.aio.get_stream(
    //    id.jellyfin_media_type.into(),
    //    id.id,
    //    stream_id
    //).await?;

    if q.static_.unwrap_or(false) {
        info!("starting direct playback for: {:?}", &media.title);
        let mut req = reqwest::Client::new().get(media.url.unwrap());
        // if let Some(axum_extra::TypedHeader(range)) = range {
        //     // let s = format!("{:?}", range);
        //     // req = req.header(axum::http::header::RANGE, s);
        //     req = req.header(http::header::RANGE, range);
        // }
        if let Some(v) = headers.get(http::header::RANGE) {
            req = req.header(http::header::RANGE, v.clone());
        }

        let upstream = req.send().await?;

        let status = upstream.status();
        let headers_in = upstream.headers().clone();
        let upstream_stream = upstream.bytes_stream();
        let body = Body::from_stream(upstream_stream.map_err(io::Error::other));

        trace!(?status, ?headers_in, "videos_stream");

        // Build outgoing response with same status
        let mut resp_out = axum::response::Response::builder()
            .status(status)
            .body(body)
            .unwrap();

        {
            use axum::http::header;
            let out_headers = resp_out.headers_mut();
            for (k, v) in headers_in.iter() {
                // Skip hop-by-hop headers
                match k.as_str().to_ascii_lowercase().as_str() {
                    "content-length" | "content-type" | "accept-ranges"
                    | "content-range" | "last-modified" => {}
                    _ => continue,
                }
                out_headers.insert(k, v.clone());
            }

            // If upstream didn't set Content-Type, default to mp4 for static direct play
            if !out_headers.contains_key(header::CONTENT_TYPE) {
                out_headers.insert(
                    header::CONTENT_TYPE,
                    header::HeaderValue::from_static("video/mp4"),
                );
            }
        }

        return Ok(resp_out);
    }

    todo!();
}

/// todo: actually implement
#[get("/playback/bitratetest")]
pub async fn playback_bitratetest(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
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
        play_method: data.play_method.clone(),
        started_at: Utc::now(),
        last_activity: Utc::now(),
    };

    ps.save(&state.ctx.store);
    info!(play_session_id, %item_id, "Playback started");

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use http::header::HeaderValue;
    use http::StatusCode;
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-progress",
                "PositionTicks": 0
            }))
            .await;

        // Report progress
        let resp = server
            .post("/sessions/playing/progress")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": "test-session-stop",
                "PositionTicks": 0
            }))
            .await;

        // Stop playback
        let resp = server
            .post("/sessions/playing/stopped")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
                .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // 4. Stop
        let resp = server
            .post("/sessions/playing/stopped")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
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
        let (server, _token) = authenticated_server().await;

        let resp = server
            .get("/sessions")
            .await;

        resp.assert_status_ok();
        let sessions: Vec<crate::jellyfin::SessionInfoDto> = resp.json();
        assert_eq!(sessions.len(), 0);
    }

    #[tokio::test]
    async fn test_get_sessions_with_active_session() {
        let (server, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Start a playback session
        let psid = "test-session-get";
        server
            .post("/sessions/playing")
            .add_header(http::header::AUTHORIZATION, HeaderValue::from_str(&auth).unwrap())
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PlaySessionId": psid,
                "PositionTicks": 0
            }))
            .await;

        // Get all sessions
        let resp = server
            .get("/sessions")
            .await;

        resp.assert_status_ok();
        let sessions: Vec<crate::jellyfin::SessionInfoDto> = resp.json();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, Some(psid.to_string()));
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
            ps.subtitle_stream_index = data.subtitle_stream_index.or(ps.subtitle_stream_index);
            ps.last_activity = Utc::now();
            ps.save(&state.ctx.store);

            // persist position to db
            let item_id = data.item_id.unwrap_or(ps.item_id);
            if let Ok(Some(media)) = db::Media::get_by_id(&state.ctx.db, &item_id).await {
                let position_seconds = ps.position_ticks / 10_000_000;
                let mut ms = db::UserMediaState::get_or_new(&state.ctx.db, &session.user, &media).await?;
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
        let final_ticks = data.position_ticks.or(ps.as_ref().map(|s| s.position_ticks));

        if let Some(item_id) = item_id {
            if let Ok(Some(media)) = db::Media::get_by_id(&state.ctx.db, &item_id).await {
                let position_seconds = final_ticks.unwrap_or(0) / 10_000_000;
                let mut ms = db::UserMediaState::get_or_new(&state.ctx.db, &session.user, &media).await?;
                ms.playback_position = position_seconds;
                // If watched to near the end (>= 90%), mark as played
                if let Some(runtime) = media.runtime {
                    let runtime_seconds = runtime;
                    if runtime_seconds > 0 && position_seconds >= (runtime_seconds * 90 / 100) {
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
    Query(q): Query<PingQuery>,
) -> Result<impl IntoResponse> {
    PlaybackSession::ping(&state.ctx.store, &q.play_session_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/capabilities/full")]
pub async fn sessions_capabilities_full(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}

/// Get all active sessions
#[get("/sessions")]
pub async fn get_sessions(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    // Get all active playback sessions using the PlaybackSession::get_all method
    let playback_sessions = PlaybackSession::get_all(&state.ctx.store);
    
    let sessions = playback_sessions
        .into_iter()
        .map(|session| {
            jellyfin::SessionInfoDto {
                id: Some(session.play_session_id.clone()),
                user_id: session.user_id.to_string(),
                user_name: None, // TODO: Get username from user ID
                client: Some(session.client_name.clone()),
                last_activity_date: session.last_activity,
                last_playback_check_in: session.last_activity,
                last_paused_date: None, // TODO: Track paused state in PlaybackSession
                device_name: Some(session.device_id.clone()),
                device_type: None,
                now_playing_item: None, // TODO: Get media info
                now_viewing_item: None,
                device_id: Some(session.device_id.clone()),
                application_version: None,
                is_active: true,
                supports_media_control: true,
                supports_remote_control: true,
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
    Ok(Json(jellyfin::UserItemDataDto::from(ms)).into_response())
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
    Ok(Json(jellyfin::UserItemDataDto::from(ms)).into_response())
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

    let play_session_id = q.play_session_id
        .unwrap_or_else(|| utils::get_uuid().as_simple().to_string());

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
        if resolved_media.kind == db::MediaKind::Movie || resolved_media.kind == db::MediaKind::Episode {
            resolved_media = resolved_media
                .sources(&state.ctx.db)
                .await?
                .get(0)
                .context_not_found("not found", "source not found")?
                .clone();
        }

        let input_url = resolved_media.url
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

        tokio::spawn(async move {
            if let Err(e) = crate::transcode::engine::start_transcode(session_clone, params).await {
                tracing::error!("Transcode failed: {:#}", e);
            }
        });

        session
    };

    // Generate and return the master playlist
    let session_read = session.read().await;
    let master_playlist = crate::transcode::engine::generate_master_playlist(&session_read);

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/vnd.apple.mpegurl")
        .header("Cache-Control", "no-cache, no-store")
        .body(Body::from(master_playlist))
        .unwrap())
}

/// Serves the variant (child) HLS playlist generated by the transcoding engine.
#[get("/videos/{id}/main/stream.m3u8")]
pub async fn variant_hls_video(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let play_session_id = q.play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    let session = state.ctx.transcode.get(&play_session_id)
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
#[get("/videos/{id}/main/{segment_id}.ts")]
pub async fn hls_segment(
    State(state): State<AppState>,
    Path((id, segment_id)): Path<(Uuid, String)>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let play_session_id = q.play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    let session = state.ctx.transcode.get(&play_session_id)
        .context_not_found("not found", "transcode session not found")?;

    // segment_id already includes the name (e.g. "segment_00001")
    // The .ts extension is part of the route pattern
    let segment_name = if segment_id.ends_with(".ts") {
        segment_id.trim_end_matches(".ts").to_string()
    } else {
        segment_id.clone()
    };

    let segment_path = session.read().await.segment_path(&segment_name);

    // Wait for the segment to be written (up to 60 seconds)
    let mut attempts = 0;
    while !segment_path.exists() && attempts < 120 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        attempts += 1;
    }

    if !segment_path.exists() {
        return Err(anyhow!("Segment {} not ready after timeout", segment_name).into());
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
#[delete("/videos/activetranscodings")]
pub async fn delete_transcoding(
    State(state): State<AppState>,
    Query(q): Query<jellyfin::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    if let Some(play_session_id) = q.play_session_id {
        info!("Stopping transcode session: {}", play_session_id);
        state.ctx.transcode.stop(&play_session_id).await;
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}
