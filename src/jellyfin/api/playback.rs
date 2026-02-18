use anyhow::Context;
use axum::Json;
use axum::body::Body;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use remux_macros::{get, post};
use axum_extra::extract::Query;
use futures_util::StreamExt;
use futures_util::TryStreamExt;
use headers;
use http::StatusCode;
use serde_json::json;
use std::io;
use tracing::info;
use tracing::trace;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
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
    Json(data): Json<jellyfin::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
#[sqlx::test]
async fn report_playback_start_test() {
    let server = crate::integration_test::new_test_server().await.unwrap();

    let response = server
        // .authorization("password12345")
        .post(&"/sessions/playing")
        .json(&json!(
        {
          "VolumeLevel": 100,
          "IsMuted": false,
          "IsPaused": false,
          "RepeatMode": "RepeatNone",
          "ShuffleMode": "Sorted",
          "MaxStreamingBitrate": 3000000,
          "PositionTicks": 10,
          "PlaybackRate": 1,
          "SubtitleStreamIndex": -1,
          "SecondarySubtitleStreamIndex": -1,
          "AudioStreamIndex": 1,
          "BufferedRanges": [],
          "PlayMethod": "Transcode",
          "PlaySessionId": "02f91e00707347bcb4366a99db7dbc74",
          "PlaylistItemId": "playlistItem0",
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

    response.assert_status_ok();
    response.assert_status_no_content();
    //response.assert_text("pong!");
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/playing/stopped")]
pub async fn report_playback_stopped(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/capabilities/full")]
pub async fn sessions_capabilities_full(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}
