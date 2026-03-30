use anyhow::anyhow;
use axum::Json;

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}
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
use tracing::{debug, info, trace};
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth;
use crate::jellyfin;
use crate::jellyfin::MediaSourceInfoExt;
use crate::playback_session::{PlaybackSession, PlaybackSessionManager};
use crate::sdks;
use crate::torrent;
use crate::transcode::session::{TranscodeSession, TranscodeState};
use crate::utils;
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

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
    items_playbackinfo_inner(
        state,
        session,
        id,
        media_source_id,
        device_profile,
        query_params,
    )
    .await
}

#[get("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<jellyfin::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    let query_params = q.clone();
    let media_source_id = q.media_source_id;
    let device_profile = q.device_profile;
    items_playbackinfo_inner(
        state,
        session,
        id,
        media_source_id,
        device_profile,
        query_params,
    )
    .await
}

async fn items_playbackinfo_inner(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
    media_source_id: Option<Uuid>,
    device_profile: Option<jellyfin::DeviceProfile>,
    query: jellyfin::PlaybackInfoQuery,
) -> Result<impl IntoResponse> {
    trace!(?id, ?media_source_id, "items_playbackinfo");

    let mut media = db::Media::get_by_id(&state.ctx.db, &media_source_id.unwrap_or(id))
        .await?
        .context_not_found("not found", "not found")?;

    // Load the top-level Movie/Episode for subtitle lookup.
    // `id` is always the movie/episode UUID; `media_source_id` may point to a
    // child Source, so we always resolve via `id` to get the IMDB fields.
    let subtitle_media = db::Media::get_by_id(&state.ctx.db, &id)
        .await
        .ok()
        .flatten();

    // Torrent streams: resolve magnet URI to a local HTTP URL first.
    let mut media = resolve_torrent(media, &state).await?;

    // IPTV channels: skip GStreamer probe, return direct-play source.
    if media.kind == db::MediaKind::TvChannel {
        let url = media
            .url
            .clone()
            .context_not_found("missing url", "channel has no stream url")?;
        let source = jellyfin::MediaSourceInfo {
            id: media.id,
            name: Some(media.title.clone()),
            path: Some(url),
            protocol: "Http".to_string(),
            is_remote: true,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: false,
            ..Default::default()
        };
        let play_session_id = utils::get_uuid().as_simple().to_string();
        let info = jellyfin::PlaybackInfoResponse {
            media_sources: vec![source],
            play_session_id: Some(play_session_id),
            ..Default::default()
        };
        return Ok(Json(info));
    }

    // Collect all playable sources. A Movie/Episode may have multiple
    // Source children (versions); return every one so the client can
    // show version selection and per-source stream lists.
    let all_source_medias: Vec<db::Media> =
        if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
            let sources = media.sources(&state.ctx.db).await?;
            if sources.is_empty() {
                // No children → treat the parent itself as the single source
                vec![media]
            } else {
                sources
            }
        } else {
            vec![media]
        };

    // When the client requests a specific source, only process that one.
    // When no source is specified (e.g. details page open), return all versions
    // for version selection but only probe the first one — probing every source
    // causes a storm of parallel FFmpeg processes (one per version of the movie).
    let (source_medias, probe_only_first) = if let Some(sid) = media_source_id {
        // Check if the source exists first before consuming the vec
        if all_source_medias.iter().any(|s| s.id == sid) {
            let filtered: Vec<db::Media> = all_source_medias
                .into_iter()
                .filter(|s| s.id == sid)
                .collect();
            (filtered, false)
        } else {
            // Requested source not found, return all without limiting probing
            (all_source_medias, false)
        }
    } else {
        (all_source_medias, true) // probe only first when no source requested
    };

    let max_bitrate: Option<i64> = match (
        query.max_streaming_bitrate,
        device_profile
            .as_ref()
            .and_then(|p| p.max_streaming_bitrate),
    ) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    let play_session_id = utils::get_uuid().as_simple().to_string();

    // Resolve all source URLs concurrently
    // (resolve_url is cheap / DB-backed so async is fine here)
    struct SourceWithUrl {
        sm: db::Media,
        resolved_url: Option<String>,
    }
    let mut sources_with_urls: Vec<SourceWithUrl> =
        Vec::with_capacity(source_medias.len());
    for sm in source_medias {
        let resolved_url = match &sm.url {
            Some(u) => Some(crate::aio::resolve_url(&state.ctx.db, u).await),
            None => None,
        };
        sources_with_urls.push(SourceWithUrl { sm, resolved_url });
    }

    // Probe all sources in parallel (spawn_blocking + timeout) ----
    // Each FFmpeg probe is a blocking CPU/network call.  Running them sequentially
    // inside the async fn blocks Tokio's runtime for 40-70 s when there are 10+
    // sources.  We launch all probes concurrently on the blocking thread pool and
    // wait for all of them together — total latency is now the slowest single probe
    // (~6-7 s) instead of the sum.
    let probe_futures: Vec<_> = sources_with_urls
        .iter()
        .enumerate()
        .map(|(idx, swu)| {
            let url_opt = swu.resolved_url.clone();
            let mut sm = swu.sm.clone();
            let db = state.ctx.db.clone();
            // When no specific source was requested, only probe the first one.
            // The rest get static metadata so we don't spawn 20+ parallel FFmpeg
            // processes just to open a details page.
            let skip_probe = probe_only_first && idx > 0;
            tokio::spawn(async move {
                if skip_probe {
                    return jellyfin::MediaSourceInfo::from(sm);
                }

                // Cache hit: deserialise stored probe result and skip FFmpeg.
                if let Some(json) = &sm.probe_data {
                    let mut cached = json.0.clone();
                    cached.id = sm.id;
                    cached.name = Some(sm.title.clone());
                    cached.path = sm.url.clone();
                    tracing::debug!(id = %sm.id, "probe cache hit");
                    return cached;
                }

                match url_opt {
                    None => jellyfin::MediaSourceInfo::from(sm),
                    Some(url) => {
                        let url2 = url.clone();
                        let sm2 = sm.clone();
                        // Wrap the blocking probe in a dedicated thread.
                        let probe_result = tokio::time::timeout(
                            std::time::Duration::from_secs(30),
                            tokio::task::spawn_blocking(move || {
                                crate::transcode::probing::probe_media(&url2)
                            }),
                        )
                        .await;

                        match probe_result {
                            // probe succeeded — persist result to cache
                            Ok(Ok(Ok(mut probed))) => {
                                probed.id = sm2.id;
                                probed.name = Some(sm2.title.clone());
                                probed.path = sm2.url.clone();
                                sm.probe_data = Some(sqlx::types::Json(probed.clone()));
                                sm.save(&db).await;
                                
                                probed
                            }
                            // probe returned an error
                            Ok(Ok(Err(e))) => {
                                tracing::warn!(url = %url, error = %e, "probe failed, falling back to static metadata");
                                jellyfin::MediaSourceInfo::from(sm2)
                            }
                            // spawn_blocking panicked
                            Ok(Err(e)) => {
                                tracing::warn!(url = %url, error = %e, "probe task panicked, falling back to static metadata");
                                jellyfin::MediaSourceInfo::from(sm2)
                            }
                            // timeout elapsed
                            Err(_) => {
                                tracing::warn!(url = %url, "probe timed out after 30s, falling back to static metadata");
                                jellyfin::MediaSourceInfo::from(sm2)
                            }
                        }
                    }
                }
            })
        })
        .collect();

    // Await all probes concurrently.
    let probed_results = futures_util::future::join_all(probe_futures).await;

    // Apply playback decision logic on the probed results ----------
    let mut media_sources = Vec::with_capacity(probed_results.len());
    for (swu, probe_join) in sources_with_urls.iter().zip(probed_results.into_iter()) {
        let sm = &swu.sm;
        let mut source: jellyfin::MediaSourceInfo =
            probe_join.unwrap_or_else(|_| jellyfin::MediaSourceInfo::from(sm.clone()));
        source.id = sm.id;
        source.e_tag = sm.id;

        // Only flag bitrate exceeded when the source bitrate is known and
        // actually exceeds the cap. An unknown bitrate is treated as within
        // limits so that clients with a high/unlimited cap aren't forced into
        // transcoding unnecessarily.
        let bitrate_exceeded =
            max_bitrate.map_or(false, |max| source.bitrate.map_or(false, |b| b > max));

        let transcode_reasons: jellyfin::TranscodeReasons = {
            let mut reasons = device_profile
                .as_ref()
                .map(|profile| profile.check_direct_play(&source))
                .unwrap_or_default();
            if bitrate_exceeded {
                reasons.insert(jellyfin::TranscodeReason::ContainerBitrateExceedsLimit);
            }
            reasons
        };

        let needs_transcoding = !transcode_reasons.is_empty()
            || query.enable_transcoding.unwrap_or(false)
            || !query.enable_direct_play.unwrap_or(true);

        tracing::debug!(
            source_id = %sm.id,
            transcode_reasons = ?transcode_reasons,
            needs_transcoding,
            bitrate_exceeded,
            "playback decision"
        );

        if needs_transcoding {
            let trans_profile = device_profile
                .as_ref()
                .and_then(|p| p.video_transcoding_profile());
            let (trans_container, trans_protocol) = trans_profile
                .map(|p| {
                    (
                        p.container.clone().unwrap_or_else(|| "ts".to_string()),
                        p.protocol.clone().unwrap_or_else(|| "hls".to_string()),
                    )
                })
                .unwrap_or_else(|| ("ts".to_string(), "hls".to_string()));

            let needs_video_transcode = transcode_reasons
                .contains(jellyfin::TranscodeReason::VideoCodecNotSupported)
                || transcode_reasons
                    .contains(jellyfin::TranscodeReason::ContainerBitrateExceedsLimit);
            let video_codec = if needs_video_transcode {
                "h264"
            } else {
                "copy"
            }
            .to_string();
            let needs_audio_transcode = transcode_reasons
                .contains(jellyfin::TranscodeReason::AudioCodecNotSupported);
            let audio_codec =
                if needs_audio_transcode { "aac" } else { "copy" }.to_string();

            let bitrate_param = max_bitrate
                .map(|b| format!("&MaxStreamingBitrate={}", b))
                .unwrap_or_default();
            let reasons_param = transcode_reasons
                .to_query_value()
                .map(|v| format!("&TranscodeReasons={}", v))
                .unwrap_or_default();
            let audio_stream_param = source
                .default_audio_stream_index
                .map(|idx| format!("&AudioStreamIndex={}", idx))
                .unwrap_or_default();

            source.supports_transcoding = true;
            source.transcoding_url = Some(format!(
                "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}",
                id,
                play_session_id,
                source.id,
                video_codec,
                audio_codec,
                bitrate_param,
                reasons_param,
                audio_stream_param,
            ));
            source.transcoding_container = Some(trans_container);
            source.transcoding_sub_protocol = trans_protocol;
            source.supports_direct_play = false;
            source.supports_direct_stream = false;
        } else {
            source.supports_transcoding = false;
            source.supports_direct_play = true;
        }

        media_sources.push(source);
    }

    // Inject external subtitles from AIO (cache-backed)
    //if let Some(ref sm) = subtitle_media {
    //     inject_external_subtitles(&state.ctx.db, sm, &mut media_sources).await;
    // }

    // Apply per-user playback preferences
    apply_user_playback_prefs(&state.ctx.db, &session.user, &id, &mut media_sources)
        .await;

    let info = jellyfin::PlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
        ..Default::default()
    };

    //trace!(?info, "items_playbackinfo_result");
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

    // IPTV channels: redirect directly to the stream URL.
    if media.kind == db::MediaKind::TvChannel {
        let url = media
            .url
            .clone()
            .context_not_found("missing url", "channel has no stream url")?;
        return Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(http::header::LOCATION, url)
            .body(Body::empty())
            .unwrap());
    }

    if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
        let sources = media.sources(&state.ctx.db).await?;
        media = if let Some(wanted) = q.media_source_id {
            sources.iter().find(|s| s.id == wanted).cloned()
        } else {
            None
        }
        .or_else(|| sources.into_iter().next())
        .context_not_found("not found", "no playable source found")?;
    }

    // Torrent streams: resolve magnet URI to a local HTTP URL.
    let media = resolve_torrent(media, &state).await?;

    let raw_url = media
        .url
        .clone()
        .context_not_found("no url", "media source has no URL")?;

    // Resolve Docker-internal hostnames to user-configured origin.
    let url = crate::aio::resolve_url(&state.ctx.db, &raw_url).await;

    // Direct play: proxy the original stream with range support.
    // Real Jellyfin always proxies the raw file bytes for Static=true, regardless
    // of stream selection parameters — the player selects tracks natively from the
    // embedded streams in the file (ExoPlayer handles HEVC/MKV natively).
    // Previously this fell into progressive transcode when AudioStreamIndex was set,
    // which broke seeking because live transcodes can't serve HTTP range requests.
    if q.static_.unwrap_or(false) {
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

    // Progressive transcode/remux: only reached when Static=false.
    let wants_stream_selection =
        q.audio_stream_index.is_some() || q.subtitle_stream_index.is_some();
    let container = q.container.as_deref().unwrap_or("mp4").to_string();
    let video_codec = q.video_codec.unwrap_or_else(|| "copy".to_string());
    let audio_codec = q.audio_codec.unwrap_or_else(|| "aac".to_string());
    // Keep a copy before the video_codec is moved into params (needed for Content-Type logic)
    let is_copy_video = video_codec == "copy";

    info!(
        "starting progressive transcode for: {:?} (container={}, vcodec={}, acodec={}, start_ticks={:?}, bitrate={:?})",
        &media.title,
        container,
        video_codec,
        audio_codec,
        q.start_time_ticks,
        q.video_bit_rate
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

    let stream = crate::transcode::engine::start_progressive_transcode(params)?;
    let body = Body::from_stream(stream);

    // The engine transparently promotes copy+mp4 → matroska (no BSF needed for Matroska).
    // Reflect that in the Content-Type so players don't get confused.
    let effective_container = if is_copy_video && container == "mp4" {
        "mkv"
    } else {
        container.as_str()
    };
    let content_type = match effective_container {
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

#[post("/sessions/logout")]
pub async fn sessions_logout(
    State(state): State<AppState>,
    session: auth::AuthSession,
) -> Result<StatusCode> {
    auth::Device::delete_by_access_token(&state.ctx.db, &session.device.access_token)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/capabilities")]
pub async fn sessions_capabilities(_session: auth::AuthSession) -> Result<StatusCode> {
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/{id}/capabilities")]
pub async fn sessions_capabilities_by_id(
    Path(_id): Path<String>,
    _session: auth::AuthSession,
) -> Result<StatusCode> {
    Ok(StatusCode::NO_CONTENT)
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
        transcode: None,
    };

    state.ctx.sessions.insert(ps);
    info!(play_session_id, %item_id, "Playback started");

    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use http::StatusCode;
    use http::header::HeaderValue;
    use serde_json::json;

    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server, insert_test_source,
        new_test_server,
    };

    #[tokio::test]
    async fn test_playback_start() {
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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
        let (server, _ctx, token) = authenticated_server().await;
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

    #[tokio::test]
    async fn test_playbackinfo_requires_auth() {
        let (server, _ctx) = new_test_server().await.unwrap();
        let fake_id = uuid::Uuid::new_v4();

        server
            .post(&format!("/items/{}/playbackinfo", fake_id))
            .expect_failure()
            .json(&json!({}))
            .await
            .assert_status(StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn test_playbackinfo_not_found() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let fake_id = uuid::Uuid::new_v4();

        server
            .post(&format!("/items/{}/playbackinfo", fake_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .json(&json!({}))
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    /// Without a device profile the endpoint always returns a transcoding URL.
    #[tokio::test]
    async fn test_playbackinfo_no_profile_returns_transcoding() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "SupportsTranscoding": true,
                "SupportsDirectPlay": false,
            }]
        }));

        let body: serde_json::Value = resp.json();
        let url = body["MediaSources"][0]["TranscodingUrl"]
            .as_str()
            .expect("TranscodingUrl should be set");
        assert!(url.contains("master.m3u8"), "should be an HLS URL: {}", url);
    }

    #[tokio::test]
    async fn test_playbackinfo_minimal() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "DeviceProfile": {
                    "DirectPlayProfiles": [
                        { "Type": "Video", "Container": "*", "VideoCodec": "*", "AudioCodec": "*" }
                    ],
                    "TranscodingProfiles": [],
                    "CodecProfiles": []
                },
                "EnableDirectPlay": true,
                "EnableTranscoding": false
            }))
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "Id": media.id.to_string(),
                "Etag": media.id.to_string(),
                "Bitrate": 3849414,
                "Container": "mp4",
             //   "Size": 292828,
                "RunTimeTicks": 100000000,
                "SupportsDirectPlay": true,
                "SupportsTranscoding": false
            }]
        }));
    }

    /// A device profile that supports direct play for the media's container
    /// causes the endpoint to return a direct-play response.
    #[tokio::test]
    async fn test_playbackinfo_direct_play_profile() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "DeviceProfile": {
                    "DirectPlayProfiles": [
                        { "Type": "Video", "Container": "*", "VideoCodec": "*", "AudioCodec": "*" }
                    ],
                    "TranscodingProfiles": [],
                    "CodecProfiles": []
                },
                "EnableDirectPlay": true,
                "EnableTranscoding": false
            }))
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "SupportsDirectPlay": true,
                "SupportsTranscoding": false,
            }]
        }));
    }

    /// When `MaxStreamingBitrate` is present the transcoding URL must include it
    /// so the HLS handler can cap the video bitrate accordingly.
    #[tokio::test]
    async fn test_playbackinfo_max_streaming_bitrate_in_url() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;
        let max_bitrate: i64 = 1_000_000;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MaxStreamingBitrate": max_bitrate }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let url = body["MediaSources"][0]["TranscodingUrl"]
            .as_str()
            .expect("TranscodingUrl should be present");
        assert!(
            url.contains(&format!("MaxStreamingBitrate={}", max_bitrate)),
            "TranscodingUrl should contain MaxStreamingBitrate: {}",
            url
        );
    }

    /// The effective bitrate is the minimum of the per-request value and the
    /// device-profile value. Both should appear in the transcoding URL.
    #[tokio::test]
    async fn test_playbackinfo_effective_bitrate_is_minimum() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        // Query says 8 Mbps, profile says 4 Mbps → effective should be 4 Mbps.
        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "MaxStreamingBitrate": 8_000_000i64,
                "DeviceProfile": {
                    "MaxStreamingBitrate": 4_000_000i64,
                    "DirectPlayProfiles": [],
                    "TranscodingProfiles": [],
                    "CodecProfiles": []
                }
            }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let url = body["MediaSources"][0]["TranscodingUrl"]
            .as_str()
            .expect("TranscodingUrl should be present");
        assert!(
            url.contains("MaxStreamingBitrate=4000000"),
            "effective bitrate should be 4 Mbps (minimum): {}",
            url
        );
    }

    /// `enable_direct_play: false` must force transcoding even with a matching
    /// direct-play profile.
    #[tokio::test]
    async fn test_playbackinfo_force_transcode_when_direct_play_disabled() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "EnableDirectPlay": false,
                "DeviceProfile": {
                    "DirectPlayProfiles": [
                        { "Type": "Video", "Container": "*" }
                    ],
                    "TranscodingProfiles": [],
                    "CodecProfiles": []
                }
            }))
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "MediaSources": [{ "SupportsTranscoding": true }]
        }));
    }

    /// Response always contains a `PlaySessionId`.
    #[tokio::test]
    async fn test_playbackinfo_has_play_session_id() {
        let (server, ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&ctx).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert!(
            body["PlaySessionId"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
            "PlaySessionId must be present and non-empty"
        );
    }
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<jellyfin::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        let ps_snapshot = state.ctx.sessions.get(psid);
        if let Some(ref ps) = ps_snapshot {
            let item_id = data.item_id.unwrap_or(ps.item_id);
            state.ctx.sessions.update(psid, |ps| {
                ps.position_ticks = data.position_ticks.unwrap_or(ps.position_ticks);
                ps.is_paused = data.is_paused;
                ps.is_muted = data.is_muted;
                ps.volume_level = data.volume_level.or(ps.volume_level);
                ps.audio_stream_index =
                    data.audio_stream_index.or(ps.audio_stream_index);
                ps.subtitle_stream_index =
                    data.subtitle_stream_index.or(ps.subtitle_stream_index);
                ps.last_activity = Utc::now();
            });

            // Update transcode buffer monitor with actual playback position.
            if let Some(position_ticks) = data.position_ticks {
                if let Some(ref ts_lock) = ps.transcode {
                    if let Ok(ts) = ts_lock.try_read() {
                        let position_secs = (position_ticks / 10_000_000) as u32;
                        let offset = position_secs.saturating_sub(ts.start_time_secs);
                        ts.playback_offset_secs
                            .store(offset, std::sync::atomic::Ordering::Relaxed);
                    }
                }
            }

            // persist position to db
            let position_ticks = data.position_ticks.unwrap_or(ps.position_ticks);
            if let Ok(Some(media)) = db::Media::get_by_id(&state.ctx.db, &item_id).await
            {
                let position_seconds = position_ticks / 10_000_000;
                let mut ms = db::UserMediaState::get_or_new(
                    &state.ctx.db,
                    &session.user,
                    &media,
                )
                .await?;
                ms.playback_position = position_seconds;
                ms.audio_idx = data
                    .audio_stream_index
                    .or(ps.audio_stream_index)
                    .map(|x| x as i64);
                ms.subtitle_idx = data
                    .subtitle_stream_index
                    .or(ps.subtitle_stream_index)
                    .map(|x| x as i64);
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
        let ps = state.ctx.sessions.stop(psid).await;

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
    state.ctx.sessions.ping(&q.play_session_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/capabilities/full")]
pub async fn sessions_capabilities_full(
    _session: auth::AuthSession,
) -> Result<StatusCode> {
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/{id}/capabilities/full")]
pub async fn sessions_capabilities_full_by_id(
    Path(_id): Path<String>,
    _session: auth::AuthSession,
) -> Result<StatusCode> {
    Ok(StatusCode::NO_CONTENT)
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
    let playback_sessions = state.ctx.sessions.get_all();

    let filtered_devices: Vec<_> = devices
        .into_iter()
        .filter(|device| {
            if let Some(cutoff) = cutoff {
                device.last_activity_at.map_or(true, |t| t >= cutoff)
            } else {
                true
            }
        })
        .collect();

    let mut sessions = Vec::with_capacity(filtered_devices.len());
    for device in filtered_devices {
        // Attach now-playing info if this device has an active playback session.
        let now_playing = playback_sessions
            .iter()
            .find(|s| s.device_id == device.id)
            .map(|s| jellyfin::BaseItemDto {
                id: s.item_id,
                ..Default::default()
            });

        // Attach TranscodingInfo if there is an active transcode session for this device.
        let transcoding_info = playback_sessions
            .iter()
            .find(|s| s.device_id == device.id)
            .and_then(|ps| ps.transcode.as_ref().and_then(|ts| ts.try_read().ok()))
            .map(|ts| jellyfin::TranscodingInfo {
                audio_codec: Some(ts.audio_codec.clone()),
                video_codec: Some(ts.video_codec.clone()),
                container: Some("ts".to_string()),
                is_video_direct: ts.video_codec == "copy",
                is_audio_direct: ts.audio_codec == "copy",
                transcode_reasons: ts.transcode_reasons.0,
                ..Default::default()
            });

        let user_name = device
            .user(&state.ctx.db)
            .await?
            .map(|u| u.username)
            .unwrap_or_default();

        sessions.push(jellyfin::SessionInfoDto {
            id: Some(device.id.clone()),
            device_id: Some(device.id.clone()),
            device_name: Some(device.name.clone()),
            client: Some(device.app_name.clone()),
            application_version: Some(device.app_version.clone()),
            user_id: device.user_id.to_string(),
            user_name: Some(user_name),
            last_activity_date: device.last_activity_at.unwrap_or_else(Utc::now),
            last_playback_check_in: device.last_activity_at.unwrap_or_else(Utc::now),
            now_playing_item: now_playing,
            transcoding_info,
            is_active: true,
            server_id: crate::utils::server_id(),
            ..Default::default()
        });
    }

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
    Ok(Json(jellyfin::db_state_to_dto(ms, id, media.runtime)).into_response())
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
    Ok(Json(jellyfin::db_state_to_dto(ms, id, media.runtime)).into_response())
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

    // Look up existing session or create a new one.
    // When the client seeks it sends the same PlaySessionId but with a new
    // StartTimeTicks.  In that case we must stop the old transcode job and
    // restart from the requested position — otherwise the player waits for
    // segments that the old job will never produce at the new offset.
    let is_seeking = q.start_time_ticks.is_some();
    if is_seeking {
        if state.ctx.sessions.get_transcode(&play_session_id).is_some() {
            tracing::debug!(
                play_session_id = %play_session_id,
                start_time_ticks = ?q.start_time_ticks,
                "seek detected — stopping old transcode session and restarting"
            );
            state.ctx.sessions.stop_transcode(&play_session_id).await;
        }
    }
    let session = if let Some(existing) =
        state.ctx.sessions.get_transcode(&play_session_id)
    {
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
            let sources = resolved_media.sources(&state.ctx.db).await?;
            resolved_media = if let Some(wanted) = q.media_source_id {
                sources.iter().find(|s| s.id == wanted).cloned()
            } else {
                None
            }
            .or_else(|| sources.into_iter().next())
            .context_not_found("not found", "no playable source found")?;
        }

        let raw_input_url = resolved_media
            .url
            .context_not_found("no url", "media source has no URL")?;

        // Resolve Docker-internal hostnames to user-configured origin.
        let input_url = crate::aio::resolve_url(&state.ctx.db, &raw_input_url).await;

        let output_dir =
            std::path::PathBuf::from("transcode_sessions").join(&play_session_id);
        let session = TranscodeSession::new(
            play_session_id.clone(),
            id,
            media_source_id,
            input_url.clone(),
            output_dir,
            video_codec.clone(),
            audio_codec.clone(),
            segment_length,
            // Parse reasons from query param (set by playbackinfo on the transcoding URL)
            q.transcode_reasons
                .as_deref()
                .map(jellyfin::TranscodeReasons::from_query_value)
                .unwrap_or_default(),
        );

        state
            .ctx
            .sessions
            .attach_transcode(&play_session_id, session.clone());

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
            // Prefer an explicit VideoBitRate; fall back to MaxStreamingBitrate so
            // the encoder targets the client-requested cap rather than CRF mode.
            video_bitrate: q
                .video_bit_rate
                .map(|v| v as u32)
                .or_else(|| q.max_streaming_bitrate.map(|b| b as u32)),
            audio_bitrate: q.audio_bit_rate.map(|v| v as u32),
            // Force stereo downmix when transcoding audio — multi-channel AAC
            // (e.g. 6.1 from DTS-HD) causes MEDIA_ERR_SRC_NOT_SUPPORTED on most
            // browsers and iOS Safari.
            audio_channels: if audio_codec == "copy" { None } else { Some(2) },
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
        .sessions
        .get_transcode(&play_session_id)
        .context_not_found("not found", "transcode session not found")?;

    let playlist_path = session.read().await.variant_playlist_path();

    // Subscribe to state changes so we can react immediately when the
    // transcoder reports an error instead of waiting the full timeout.
    let mut state_rx = session.read().await.state_tx.subscribe();

    // Wait up to 30 seconds for the variant playlist to appear, waking
    // immediately on every state-change broadcast from the engine.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        if playlist_path.exists() {
            break;
        }
        if let TranscodeState::Error(ref msg) = *state_rx.borrow() {
            return Err(anyhow!("Transcode failed: {}", msg).into());
        }
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            return Err(anyhow!("Variant playlist not ready after timeout").into());
        }
        // Wake on state change or after a short poll interval (whichever is first).
        tokio::select! {
            _ = state_rx.changed() => {
                if let TranscodeState::Error(ref msg) = *state_rx.borrow() {
                    return Err(anyhow!("Transcode failed: {}", msg).into());
                }
            }
            _ = tokio::time::sleep_until(deadline.min(
                tokio::time::Instant::now() + std::time::Duration::from_millis(500)
            )) => {}
        }
    }

    // Keep the session alive.
    state.ctx.sessions.ping(&play_session_id);

    let raw = tokio::fs::read_to_string(&playlist_path).await?;

    // FFmpeg writes bare filenames (e.g. `segment_00001.ts`). Inject
    // PlaySessionId so the segment handler can look up the transcode session.
    let content = raw
        .lines()
        .map(|line| {
            if !line.starts_with('#') && line.ends_with(".ts") {
                format!("{}?PlaySessionId={}", line, play_session_id)
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n");

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

/// Segment route at the same level as main.m3u8 — browsers resolve bare
/// segment filenames relative to the variant playlist URL.
#[get("/videos/{id}/{segment_file}")]
pub async fn hls_segment_flat(
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

    let session = state.ctx.sessions.get_transcode(&play_session_id);

    // Derive the segment path — either from the live session or from the base
    // dir directly (handles server restart where session is gone but files remain).
    let segment_path = match &session {
        Some(s) => s.read().await.segment_path(&segment_id),
        None => state
            .ctx
            .sessions
            .segment_path(&play_session_id, &segment_id),
    };

    if let Some(ref session) = session {
        // Update playback position for the buffer monitor.
        if let Some(idx) = segment_id
            .rsplit('_')
            .next()
            .and_then(|n| n.parse::<u32>().ok())
        {
            use std::sync::atomic::Ordering;
            let s = session.read().await;
            let prev = s.last_segment_index.load(Ordering::Relaxed);
            if idx > prev {
                s.last_segment_index.store(idx, Ordering::Relaxed);
            }
        }
    }

    // If the session is live, wait up to 60s for ffmpeg to produce the segment.
    // If there's no live session (e.g. after server restart), only serve from disk.
    if session.is_some() {
        let mut attempts = 0;
        while !segment_path.exists() && attempts < 120 {
            tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            attempts += 1;
        }
    }

    if !segment_path.exists() {
        if session.is_none() {
            None::<()>.context_not_found(
                "not found",
                &format!(
                    "transcode session {} gone and segment {} not on disk",
                    play_session_id, segment_id
                ),
            )?;
        }
        None::<()>.context_not_found(
            "not found",
            &format!("segment {} not ready after timeout", segment_id),
        )?;
    }

    // Keep the session alive — the segment request counts as activity.
    state.ctx.sessions.ping(&play_session_id);

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
        state.ctx.sessions.stop_transcode(&play_session_id).await;
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

/// If `media.url` is a magnet URI, resolve it via the torrent manager to a local
/// HTTP URL and return a clone with the resolved URL.  For all other URLs this is
/// a no-op that returns the original `media` unchanged.
async fn resolve_torrent(mut media: db::Media, state: &AppState) -> Result<db::Media> {
    let url = match media.url.as_deref() {
        Some(u) if u.starts_with("magnet:") => u.to_owned(),
        _ => return Ok(media),
    };
    // Check whether P2P is enabled in server config.
    let cfg = crate::db::Settings::get_config(&state.ctx.db).await?;
    if !cfg.p2p_enabled.unwrap_or(true) {
        return Err(anyhow::anyhow!("P2P streams are disabled")).context_bad_request(
            "torrent",
            "P2P streams are disabled by the server administrator",
        );
    }
    let resolved = state
        .ctx
        .torrent
        .resolve_url(&url)
        .await
        .context_bad_request("torrent", "failed to resolve torrent stream")?;
    media.url = Some(resolved);
    Ok(media)
}

/// Subtitle extraction endpoint - extracts a subtitle stream from a media source
/// and optionally converts it to the requested format (vtt, srt, ass).
#[get("/videos/{item_id}/{media_source_id}/subtitles/{stream_index}/stream.{format}")]
pub async fn subtitles_stream(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((item_id, media_source_id, stream_index, format)): Path<(
        Uuid,
        Uuid,
        i64,
        String,
    )>,
) -> Result<impl IntoResponse> {
    let _ = item_id; // Jellyfin API includes item_id in the path but we only need media_source_id

    let mut media = db::Media::get_by_id(&state.ctx.db, &media_source_id)
        .await?
        .context_not_found("not found", "media source not found")?;

    if media.kind == db::MediaKind::Movie || media.kind == db::MediaKind::Episode {
        media = media
            .sources(&state.ctx.db)
            .await?
            .get(0)
            .context_not_found("not found", "no sources found")?
            .clone();
    }

    let url = media
        .url
        .clone()
        .context_not_found("no url", "media source has no URL")?;

    let output_format = format.to_ascii_lowercase();
    let (ffmpeg_format, content_type) = match output_format.as_str() {
        "vtt" | "webvtt" => ("webvtt", "text/vtt; charset=utf-8"),
        "srt" | "subrip" => ("srt", "text/plain; charset=utf-8"),
        "ass" | "ssa" => ("ass", "text/plain; charset=utf-8"),
        "pgssub" | "sup" => ("sup", "application/octet-stream"),
        _ => ("srt", "text/plain; charset=utf-8"),
    };

    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.args([
        "-i",
        &url,
        "-map",
        &format!("0:{stream_index}"),
        "-c:s",
        if ffmpeg_format == "sup" {
            "copy"
        } else {
            ffmpeg_format
        },
        "-f",
        ffmpeg_format,
        "-",
    ]);
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let child = cmd
        .spawn()
        .map_err(|e| anyhow!("failed to spawn ffmpeg: {e}"))?;
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| anyhow!("ffmpeg failed: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        tracing::error!("ffmpeg subtitle extraction failed: {stderr}");
        return Ok(Response::builder()
            .status(StatusCode::INTERNAL_SERVER_ERROR)
            .body(Body::from("subtitle extraction failed"))
            .unwrap());
    }

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=3600")
        .header("Access-Control-Allow-Origin", "*")
        .body(Body::from(output.stdout))
        .unwrap())
}

/// Inject external subtitles from AIO into a list of `MediaSourceInfo` entries.
/// Delegates to [`crate::db::Media::inject_subtitles_into_sources`].
pub(super) async fn inject_external_subtitles(
    db: &sqlx::SqlitePool,
    subtitle_media: &crate::db::Media,
    media_sources: &mut Vec<jellyfin::MediaSourceInfo>,
) {
    subtitle_media
        .inject_subtitles_into_sources(db, media_sources)
        .await;
}

/// Apply per-user playback preferences to a list of `MediaSourceInfo` entries:
///
/// - **`remember_audio_selections`**: restore the last-used audio stream index as the
///   default (only if that stream still exists in the probed list).
/// - **`remember_subtitle_selections`**: same for subtitles.
/// - **`play_default_audio_track`**: if `false`, clear the default audio stream index
///   so the client plays without auto-selecting audio (after recall is applied).
/// - **`subtitle_language_preference`**: if set and no subtitle is yet marked as
///   default, find the first subtitle stream whose language matches (normalised to
///   ISO 639-1) and mark it.
async fn apply_user_playback_prefs(
    db: &sqlx::SqlitePool,
    user: &crate::db::User,
    media_id: &uuid::Uuid,
    media_sources: &mut Vec<jellyfin::MediaSourceInfo>,
) {
    let cfg = user
        .configuration
        .as_ref()
        .map(|c| c.0.clone())
        .unwrap_or_default();

    // Load saved stream selections (best-effort; failure means no recall)
    let user_state = crate::db::Media::get_by_id(db, media_id)
        .await
        .ok()
        .flatten()
        .and_then(|m| {
            // We only have an async get_or_new, so do a sync-compatible lookup inline
            // via the aio_id key used by UserMediaState.
            m.media_id.map(|key| (key, m.kind))
        })
        .and_then(|(key, _kind)| {
            // We can't `.await` inside an `and_then`, so capture key and fetch below.
            Some(key)
        });

    let saved_audio: Option<i64>;
    let saved_subtitle: Option<i64>;

    if let Some(media_key) = user_state {
        match sqlx::query_as::<_, crate::db::UserMediaState>(
            "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_key = ?2",
        )
        .bind(user.id)
        .bind(&media_key)
        .fetch_optional(db)
        .await
        {
            Ok(Some(state)) => {
                saved_audio = state.audio_idx;
                saved_subtitle = state.subtitle_idx;
            }
            _ => {
                saved_audio = None;
                saved_subtitle = None;
            }
        }
    } else {
        saved_audio = None;
        saved_subtitle = None;
    }

    for source in media_sources.iter_mut() {
        // --- remember_audio_selections ---
        if cfg.remember_audio_selections {
            if let Some(idx) = saved_audio {
                let exists = source.media_streams.iter().any(|s| {
                    s.index == Some(idx)
                        && matches!(s.type_, Some(jellyfin::MediaStreamType::Audio))
                });
                if exists {
                    source.default_audio_stream_index = Some(idx);
                }
            }
        }

        // --- remember_subtitle_selections ---
        if cfg.remember_subtitle_selections {
            if let Some(idx) = saved_subtitle {
                let exists = source.media_streams.iter().any(|s| {
                    s.index == Some(idx)
                        && matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle))
                });
                if exists {
                    // Clear any previous default flag, set the recalled one
                    for s in source.media_streams.iter_mut() {
                        if matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle))
                        {
                            s.is_default = Some(false);
                        }
                    }
                    source.default_subtitle_stream_index = Some(idx);
                    if let Some(s) = source
                        .media_streams
                        .iter_mut()
                        .find(|s| s.index == Some(idx))
                    {
                        s.is_default = Some(true);
                    }
                }
            }
        }

        // --- play_default_audio_track ---
        if !cfg.play_default_audio_track {
            source.default_audio_stream_index = None;
        }

        // --- subtitle_language_preference ---
        // Only act if no subtitle default is already set
        if source.default_subtitle_stream_index.is_none() {
            if let Some(ref pref) = cfg.subtitle_language_preference {
                let pref_two = crate::db::subtitle_lang_to_two_letter(pref);
                if let Some(ref target) = pref_two {
                    if let Some(stream) = source.media_streams.iter_mut().find(|s| {
                        matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle))
                            && s.language
                                .as_deref()
                                .and_then(crate::db::subtitle_lang_to_two_letter)
                                .as_deref()
                                == Some(target.as_str())
                    }) {
                        let idx = stream.index;
                        stream.is_default = Some(true);
                        source.default_subtitle_stream_index = idx;
                    }
                }
            }
        }

        // --- subtitle_mode ---
        apply_subtitle_mode(&cfg.subtitle_mode, source);
    }
}

fn apply_subtitle_mode(
    mode: &jellyfin::SubtitleMode,
    source: &mut jellyfin::MediaSourceInfo,
) {
    let clear_all = |source: &mut jellyfin::MediaSourceInfo| {
        for s in source.media_streams.iter_mut() {
            if matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = None;
    };

    let set_default = |source: &mut jellyfin::MediaSourceInfo, idx: Option<i64>| {
        for s in source.media_streams.iter_mut() {
            if matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = idx;
        if let Some(i) = idx {
            if let Some(s) =
                source.media_streams.iter_mut().find(|s| s.index == Some(i))
            {
                s.is_default = Some(true);
            }
        }
    };

    match mode {
        jellyfin::SubtitleMode::None => {
            // Never auto-show subtitles
            clear_all(source);
        }
        jellyfin::SubtitleMode::Always => {
            // If no subtitle is already selected, pick the first non-forced subtitle
            if source.default_subtitle_stream_index.is_none() {
                let idx = source.media_streams.iter().find_map(|s| {
                    if matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle))
                        && !s.is_forced.unwrap_or(false)
                    {
                        s.index
                    } else {
                        None
                    }
                });
                if idx.is_some() {
                    set_default(source, idx);
                }
            }
        }
        jellyfin::SubtitleMode::OnlyForced => {
            // Only a forced subtitle may be default; clear any non-forced default
            let forced_idx = source.media_streams.iter().find_map(|s| {
                if matches!(s.type_, Some(jellyfin::MediaStreamType::Subtitle))
                    && s.is_forced.unwrap_or(false)
                {
                    s.index
                } else {
                    None
                }
            });
            // Replace whatever is set with the first forced sub (or nothing)
            set_default(source, forced_idx);
        }
        jellyfin::SubtitleMode::Smart => {
            // Like Default but clear the selection if the subtitle language already
            // matches the audio language (i.e. no translation needed).
            if let Some(def_idx) = source.default_subtitle_stream_index {
                let audio_lang = source
                    .media_streams
                    .iter()
                    .find(|s| {
                        matches!(s.type_, Some(jellyfin::MediaStreamType::Audio))
                            && s.index == source.default_audio_stream_index
                    })
                    .and_then(|s| s.language.clone());

                let sub_lang = source
                    .media_streams
                    .iter()
                    .find(|s| s.index == Some(def_idx))
                    .and_then(|s| s.language.clone());

                let audio_two = audio_lang
                    .as_deref()
                    .and_then(crate::db::subtitle_lang_to_two_letter);
                let sub_two = sub_lang
                    .as_deref()
                    .and_then(crate::db::subtitle_lang_to_two_letter);

                if audio_two.is_some() && audio_two == sub_two {
                    // Subtitle language matches audio — no need to display it
                    clear_all(source);
                }
            }
        }
        // Default: do not alter what was already set by prior steps
        jellyfin::SubtitleMode::Default => {}
    }
}
