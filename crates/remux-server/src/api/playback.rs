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
use url::Url;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::api::MediaSourceInfoExt;
use crate::common;
use crate::db;
use crate::db::auth;
use crate::playback_session::{PlaybackSession, PlaybackSessionManager};
use crate::profile::DeviceProfileExt;
use crate::sdks;
use crate::torrent;
use crate::transcode::session::{TranscodeSession, TranscodeState};
use axum_anyhow::{ApiResult as Result, OptionExt, ResultExt};

/// Some Stremio addons expose `aiostreams` as an internal hostname not
/// resolvable from the Remux process. If the user has at least one
/// `kind=stremio` addon configured, rewrite that hostname to the addon's
/// origin. Returns the URL unchanged when no rewrite applies.

#[post("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Json(payload): Json<api::PlaybackInfoQuery>,
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
    Query(q): Query<api::PlaybackInfoQuery>,
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
    device_profile: Option<api::DeviceProfile>,
    query: api::PlaybackInfoQuery,
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

    let is_live = media.kind == db::MediaKind::TvChannel;

    let is_track = media.kind == db::MediaKind::Track
        || subtitle_media
            .as_ref()
            .map_or(false, |m| m.kind == db::MediaKind::Track);
    let has_lyrics = is_track;

    // Collect all playable sources. A Movie/Episode may have multiple
    // Source children (versions); return every one so the client can
    // show version selection and per-source stream lists.
    let all_source_medias: Vec<db::Media> = if matches!(
        media.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        let sources = media.streams(&state.ctx.db).await?;
        if sources.is_empty() {
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
    //
    // Android TV always sends media_source_id = item_id (not None) for auto-play.
    // Treat that the same as "return first source only" — no need to send all versions.
    let specific_source_requested = media_source_id
        .map(|sid| sid != id && all_source_medias.iter().any(|s| s.id == sid))
        .unwrap_or(false);
    let (source_medias, probe_only_first) = if specific_source_requested {
        // Specific stream requested: return only that stream.
        let sid = media_source_id.unwrap();
        let filtered: Vec<db::Media> = all_source_medias
            .into_iter()
            .filter(|s| s.id == sid)
            .collect();
        (filtered, false)
    } else if media_source_id.is_some() {
        // media_source_id provided but equals item_id (Android TV auto-play) or
        // stream not found: return only the first (best) source.
        // specific_source_requested stays false so source[0].id is overridden to
        // item_id below — required for Android TV routing.
        let mut v = all_source_medias;
        v.truncate(1);
        (v, false)
    } else {
        // No source ID: return all versions for the selection UI,
        // probe only the first to avoid spawning N FFmpeg processes.
        (all_source_medias, true)
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

    let play_session_id = common::get_uuid().as_simple().to_string();

    struct SourceWithUrl {
        sm: db::Media,
        resolved_url: Option<String>,
    }
    let mut sources_with_urls: Vec<SourceWithUrl> =
        Vec::with_capacity(source_medias.len());
    let port = state.ctx.config.port;
    for sm in source_medias {
        let resolved_url = sm.url.as_ref().map(|d| d.server_input(sm.id, port));
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
                // Cache hit: deserialise stored probe result and skip FFmpeg.
                if let Some(json) = &sm.probe_data {
                    let mut cached = json.clone();
                    // Discard cached probe if it has no video stream — it's incomplete.
                    if cached.video_stream().is_some() {
                        cached.id = sm.id;
                        cached.name = Some(sm.title.clone());
                        cached.path = sm.url.as_ref().and_then(|d| d.as_http_url().map(str::to_owned));
                        tracing::debug!(id = %sm.id, "probe cache hit");
                        return cached;
                    }
                    tracing::debug!(id = %sm.id, "probe cache stale (no video stream), re-probing");
                }

                if skip_probe {
                    return api::MediaSourceInfo::from(sm);
                }

                match url_opt {
                    None => api::MediaSourceInfo::from(sm),
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
                            // probe succeeded — persist result to cache only if it has a video stream
                            Ok(Ok(Ok((mut probed, segments)))) => {
                                probed.id = sm2.id;
                                probed.name = Some(sm2.title.clone());
                                probed.path = sm2.url.as_ref().and_then(|d| d.as_http_url().map(str::to_owned));
                                if probed.video_stream().is_some() || probed.audio_stream().is_some() {
                                    if !segments.is_empty() {
                                        probed.segments = Some(segments);
                                    }
                                    if let Err(e) = db::Media::save_probe_data(&db, &sm2.id, &probed).await {
                                        tracing::warn!(id = %sm2.id, error = %e, "failed to save probe data");
                                    }
                                } else {
                                    tracing::warn!(id = %sm2.id, "probe returned no audio or video stream, not caching");
                                }

                                probed
                            }
                            // probe returned an error
                            Ok(Ok(Err(e))) => {
                                tracing::warn!(url = %url, error = %e, "probe failed, falling back to static metadata");
                                api::MediaSourceInfo::from(sm2)
                            }
                            // spawn_blocking panicked
                            Ok(Err(e)) => {
                                tracing::warn!(url = %url, error = %e, "probe task panicked, falling back to static metadata");
                                api::MediaSourceInfo::from(sm2)
                            }
                            // timeout elapsed
                            Err(_) => {
                                tracing::warn!(url = %url, "probe timed out after 30s, falling back to static metadata");
                                api::MediaSourceInfo::from(sm2)
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
        let mut source: api::MediaSourceInfo =
            probe_join.unwrap_or_else(|_| api::MediaSourceInfo::from(sm.clone()));
        source.id = sm.id;
        source.e_tag = sm.id;
        source.has_segments = true;
        // Use the AIO-remapped URL so clients can reach it for direct play.
        if let Some(ref resolved) = swu.resolved_url {
            source.path = Some(resolved.clone());
        }

        // Re-apply binge-group headers on top of the probed result —
        // ffmpeg probing produces a fresh `MediaSourceInfo` and would
        // otherwise drop the `X-Remux-BingeGroup` / `X-Gelato-BingeGroup`
        // hints we stashed alongside the source.
        source.remux = Some(api::MediaSourceRemuxInfo {
            provider_info: sm
                .provider_info
                .clone()
                .and_then(|info| serde_json::to_value(info).ok()),
        });

        if has_lyrics {
            api::inject_lyric_stream(&mut source);
        }

        // Only flag bitrate exceeded when the source bitrate is known and
        // actually exceeds the cap. An unknown bitrate is treated as within
        // limits so that clients with a high/unlimited cap aren't forced into
        // transcoding unnecessarily.
        let bitrate_exceeded =
            max_bitrate.map_or(false, |max| source.bitrate.map_or(false, |b| b > max));

        let mut transcode_reasons: api::TranscodeReasons = {
            let mut reasons = device_profile
                .as_ref()
                .map(|profile| profile.check_direct_play(&source))
                .unwrap_or_default();
            if bitrate_exceeded {
                reasons.insert(api::TranscodeReason::ContainerBitrateExceedsLimit);
            }
            reasons
        };

        // Image-based subtitles (PGS/DVD) can't be rendered by web clients — detect
        // from the explicitly-selected or default subtitle stream and add a transcode reason.
        let effective_sub_idx = query
            .subtitle_stream_index
            .or(source.default_subtitle_stream_index);
        let needs_pgs_burn = effective_sub_idx.map_or(false, |idx| {
            source.media_streams.iter().any(|s| {
                s.index == idx
                    && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                    && matches!(
                        s.codec.as_deref().unwrap_or(""),
                        "pgssub" | "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvdsub"
                    )
            })
        });
        if needs_pgs_burn {
            let codec = effective_sub_idx
                .and_then(|idx| source.media_streams.iter().find(|s| s.index == idx))
                .and_then(|s| s.codec.clone())
                .unwrap_or_default();
            transcode_reasons
                .insert(api::TranscodeReason::SubtitleCodecNotSupported(codec));
        }

        // `EnableTranscoding=true` means "allowed", not "forced".
        let transcode_required = !transcode_reasons.is_empty()
            || !query.enable_direct_play.unwrap_or(true)
            || !query.enable_direct_stream.unwrap_or(true);
        let needs_transcoding =
            transcode_required && query.enable_transcoding.unwrap_or(true);

        tracing::debug!(
            source_id = %sm.id,
            transcode_reasons = ?transcode_reasons,
            "playback decision"
        );

        if needs_transcoding {
            let is_audio_only = source.video_stream().is_none();

            if is_audio_only {
                let trans_profile = device_profile
                    .as_ref()
                    .and_then(|p| p.audio_transcoding_profile());
                let trans_container = trans_profile
                    .and_then(|p| p.container.clone())
                    .unwrap_or_else(|| "mp3".to_string());
                let audio_codec = trans_profile
                    .and_then(|p| p.audio_codec.as_deref())
                    .and_then(|c| c.split(',').next())
                    .map(|c| c.trim().to_string())
                    .unwrap_or_else(|| "aac".to_string());

                let start_time_param = query
                    .start_time_ticks
                    .map(|t| format!("&StartTimeTicks={}", t))
                    .unwrap_or_default();

                source.supports_transcoding = true;
                source.transcoding_url = Some(format!(
                    "/videos/{}/stream.{}?MediaSourceId={}&AudioCodec={}{}&ApiKey={}",
                    id,
                    trans_container,
                    source.id,
                    audio_codec,
                    start_time_param,
                    session.device.access_token,
                ));
                source.transcoding_container = Some(trans_container);
                source.transcoding_sub_protocol = "http".to_string();
                source.supports_direct_play = false;
                source.supports_direct_stream = false;
            } else {
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

                let needs_video_transcode =
                    transcode_reasons.contains(
                        &api::TranscodeReason::VideoCodecNotSupported(String::new()),
                    ) || transcode_reasons
                        .contains(&api::TranscodeReason::ContainerBitrateExceedsLimit)
                        || transcode_reasons.contains(
                            &api::TranscodeReason::VideoRangeTypeNotSupported(
                                String::new(),
                            ),
                        );
                let mut video_codec = if needs_video_transcode {
                    "h264"
                } else {
                    "copy"
                }
                .to_string();
                let needs_audio_transcode = transcode_reasons.contains(
                    &api::TranscodeReason::AudioCodecNotSupported(String::new()),
                );
                let audio_codec =
                    if needs_audio_transcode { "aac" } else { "copy" }.to_string();

                // Detect image-based subtitle streams (PGS, DVD) that cannot be
                // embedded in HLS — burn them into the video via FFmpeg overlay.
                let selected_sub_idx = effective_sub_idx;
                let subtitle_method = selected_sub_idx
                    .and_then(|idx| {
                        source.media_streams.iter().find(|s| {
                            s.index == idx
                                && matches!(
                                    s.type_,
                                    Some(api::MediaStreamType::Subtitle)
                                )
                        })
                    })
                    .and_then(|stream| {
                        let codec = stream.codec.as_deref().unwrap_or("");
                        let is_image_sub = matches!(
                            codec,
                            "pgssub" | "hdmv_pgs_subtitle" | "dvd_subtitle"
                        );
                        if !is_image_sub {
                            return None;
                        }
                        Some(
                            device_profile
                                .as_ref()
                                .and_then(|p| p.subtitle_delivery_method(codec))
                                .unwrap_or(api::SubtitleDeliveryMethod::Encode),
                        )
                    });

                if subtitle_method == Some(api::SubtitleDeliveryMethod::Encode) {
                    video_codec = "h264".to_string();
                }

                let bitrate_param = max_bitrate
                    .map(|b| format!("&MaxStreamingBitrate={}", b))
                    .unwrap_or_default();
                let reasons_param = transcode_reasons
                    .to_query_value()
                    .map(|v| format!("&TranscodeReasons={}", v))
                    .unwrap_or_default();
                let audio_stream_param = query
                    .audio_stream_index
                    .or(source.default_audio_stream_index)
                    .map(|idx| format!("&AudioStreamIndex={}", idx))
                    .unwrap_or_default();
                let subtitle_stream_param = selected_sub_idx
                    .map(|idx| format!("&SubtitleStreamIndex={}", idx))
                    .unwrap_or_default();
                let subtitle_method_param = subtitle_method
                    .map(|m| format!("&SubtitleMethod={}", m))
                    .unwrap_or_default();
                let start_time_param = query
                    .start_time_ticks
                    .map(|t| format!("&StartTimeTicks={}", t))
                    .unwrap_or_default();

                source.supports_transcoding = true;
                source.transcoding_url = Some(format!(
                    "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}&ApiKey={}",
                    id,
                    play_session_id,
                    source.id,
                    video_codec,
                    audio_codec,
                    bitrate_param,
                    reasons_param,
                    audio_stream_param,
                    subtitle_stream_param,
                    subtitle_method_param,
                    start_time_param,
                    session.device.access_token,
                ));
                source.transcoding_container = Some(trans_container);
                source.transcoding_sub_protocol = trans_protocol;
                source.supports_direct_play = false;
                source.supports_direct_stream = false;
            }
        } else {
            // Keep transcoding available so clients can re-request with a subtitle
            // index (e.g. PGS burn-in) even when direct-play is otherwise fine.
            source.supports_transcoding = true;
            source.supports_direct_play = true;
            // Route track direct play through the server (CDN URLs are IP-restricted).
            if is_track {
                source.path = Some(format!(
                    "/audio/{}/stream?Static=true&MediaSourceId={}",
                    id, source.id
                ));
            }
        }

        // Set delivery URL on text subtitle streams so clients can download them.
        let source_id = source.id;
        let api_key = &session.device.access_token;
        for stream in source.media_streams.iter_mut() {
            if stream.type_ != Some(api::MediaStreamType::Subtitle) {
                continue;
            }
            if !stream.is_text_subtitle_stream {
                continue;
            }
            // Embedded text subs: Embed delivery — MPV reads from container.
            // No URL needed; External would cause MPV to double-load the track.
            stream.delivery_method = Some(api::SubtitleDeliveryMethod::Embed);
        }

        source.transcoding_reasons = transcode_reasons;
        media_sources.push(source);
    }

    // Inject external subtitles from AIO (cache-backed)
    if let Some(ref sm) = subtitle_media {
        inject_external_subtitles(
            &state.ctx,
            sm,
            &mut media_sources,
            id,
            &session.device.access_token,
        )
        .await;
    }

    // Apply per-user playback preferences
    apply_user_playback_prefs(&state.ctx.db, &session.user, &id, &mut media_sources)
        .await;

    // When no specific source was requested (initial load, or media_source_id == item_id),
    // override source[0].Id to equal the item ID — clients expect this for auto-play.
    // When a real specific source was requested, keep its UUID so the client
    // sends it back and we resolve the right stream.
    if !specific_source_requested && !media_sources.is_empty() {
        media_sources[0].id = id;
        media_sources[0].e_tag = id;
    }

    // Live TV: apply stream flags on top of whatever the probe/transcoding decided.
    if is_live {
        for source in &mut media_sources {
            source.is_infinite_stream = true;
            source.ignore_dts = true;
            source.ignore_index = true;
            source.read_at_native_framerate = true;
            source.buffer_ms = Some(1500);
            source.run_time_ticks = None;
        }
    }

    let info = api::PlaybackInfoResponse {
        media_sources,
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
#[get("/items/{id}/file", "/items/{id}/download")]
pub async fn items_file(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(mut q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    q.static_ = Some(true);
    videos_stream_inner(headers, state, id, q).await
}

/// # Static
///
/// If the `static_` query parameter is set to `true`, the response will be a static
/// video stream. Otherwise, a progressive transcode is started.
#[get("/audio/{id}/stream")]
pub async fn audio_stream(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    videos_stream_inner(headers, state, id, q).await
}

#[get("/audio/{id}/stream.{container}")]
pub async fn audio_stream_by_container(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path((id, container)): Path<(Uuid, String)>,
    Query(mut q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    if q.container.is_none() {
        q.container = Some(container);
    }
    videos_stream_inner(headers, state, id, q).await
}

#[get("/videos/{id}/stream")]
pub async fn videos_stream(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    videos_stream_inner(headers, state, id, q).await
}

#[get("/videos/{id}/stream.{container}")]
pub async fn videos_stream_by_container(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path((id, container)): Path<(Uuid, String)>,
    Query(mut q): Query<api::VideoStreamQuery>,
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
    q: api::VideoStreamQuery,
) -> Result<impl IntoResponse> {
    let mut media =
        db::Media::get_by_id(&state.ctx.db, &q.media_source_id.unwrap_or(id))
            .await?
            .context_not_found("not found", "not found")?;

    // IPTV channels: redirect directly to the stream URL.
    if media.kind == db::MediaKind::TvChannel {
        let url = media
            .url
            .as_ref()
            .and_then(|d| d.as_http_url().map(str::to_owned))
            .context_not_found("missing url", "channel has no stream url")?;
        return Ok(Response::builder()
            .status(StatusCode::FOUND)
            .header(http::header::LOCATION, url)
            .body(Body::empty())
            .unwrap());
    }

    if media.kind == db::MediaKind::Movie
        || media.kind == db::MediaKind::Episode
        || media.kind == db::MediaKind::Track
    {
        let sources = media.streams(&state.ctx.db).await?;
        media = if let Some(wanted) = q.media_source_id {
            sources.iter().find(|s| s.id == wanted).cloned()
        } else {
            None
        }
        .or_else(|| sources.into_iter().next())
        .context_not_found("not found", "no playable source found")?;
    }

    let descriptor = media
        .url
        .clone()
        .context_not_found("no url", "media source has no URL")?;

    // Direct play: serve bytes directly through the StreamSource trait.
    // This handles HTTP, local files, torrents, and opendal without going through
    // our own HTTP proxy — TorrentSource resolves and streams inline.
    if q.static_.unwrap_or(false) {
        let resp = if let Some(addon_id) = descriptor.addon_id() {
            let addon = state
                .ctx
                .addons
                .get(addon_id)
                .await
                .context_not_found("stream", "addon not found")?;
            addon.kind.serve_stream(&descriptor, &headers).await?
        } else {
            descriptor
                .clone()
                .into_source()
                .serve(&state, &headers)
                .await?
        };
        return Ok(resp);
    }

    let url = descriptor.server_input(media.id, state.ctx.config.port);

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

    let encoding_opts = crate::db::Settings::get_encoding_config(&state.ctx.db)
        .await
        .unwrap_or_default();
    let source_video_codec = media
        .probe_data
        .as_ref()
        .and_then(|p| p.video_stream())
        .and_then(|s| s.codec.clone());
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
        burn_subtitle: q.subtitle_method.as_deref() == Some("Encode"),
        subtitle_width: None,
        subtitle_height: None,
        encoding_preset: encoding_opts.encoding_preset,
        source_video_codec,
        hardware_acceleration_type: encoding_opts
            .hardware_acceleration_type
            .unwrap_or_default(),
        vaapi_device: encoding_opts
            .vaapi_device
            .unwrap_or_else(|| "/dev/dri/renderD128".to_string()),
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
pub async fn sessions_capabilities(
    State(state): State<AppState>,
    session: auth::AuthSession,
    body: Option<Json<api::ClientCapabilitiesDto>>,
) -> Result<StatusCode> {
    if let Some(Json(caps)) = body {
        let _ =
            auth::Device::save_capabilities(&state.ctx.db, &session.device.id, &caps)
                .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/{id}/capabilities")]
pub async fn sessions_capabilities_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _session: auth::AuthSession,
    body: Option<Json<api::ClientCapabilitiesDto>>,
) -> Result<StatusCode> {
    if let Some(Json(caps)) = body {
        let _ = auth::Device::save_capabilities(&state.ctx.db, &id, &caps).await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/playing")]
pub async fn report_playback_start(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackStartInfo>,
) -> Result<impl IntoResponse> {
    let play_session_id = data
        .play_session_id
        .clone()
        .unwrap_or_else(|| common::get_uuid().as_simple().to_string());

    let item_id = data.item_id.unwrap_or_default();

    let ps = PlaybackSession {
        play_session_id: play_session_id.clone(),
        user_id: session.user.id,
        item_id,
        media_source_id: data.media_source_id.clone(),
        device_id: session.device.id.clone(),
        client_name: session.device.app_name.clone(),
        position_ticks: data.position_ticks.unwrap_or(0),
        can_seek: data.can_seek,
        is_paused: data.is_paused,
        last_paused_at: if data.is_paused {
            Some(Utc::now())
        } else {
            None
        },
        is_muted: data.is_muted,
        volume_level: data.volume_level,
        audio_stream_index: data.audio_stream_index,
        subtitle_stream_index: data.subtitle_stream_index,
        play_method: data.play_method.as_ref().map(|m| m.to_string()),
        now_playing_queue: data.now_playing_queue.clone(),
        playlist_item_id: data.playlist_item_id.clone(),
        started_at: Utc::now(),
        last_activity: Utc::now(),
        transcode: None,
    };

    state.ctx.sessions.insert(ps);
    let media_title = db::Media::get_by_id(&state.ctx.db, &item_id)
        .await
        .ok()
        .flatten()
        .map(|m| m.title)
        .unwrap_or_default();
    let (source_title, source_path) = if let Some(ref sid) = data.media_source_id {
        if let Ok(source_uuid) = sid.parse::<Uuid>() {
            let m = db::Media::get_by_id(&state.ctx.db, &source_uuid)
                .await
                .ok()
                .flatten();
            (m.as_ref().map(|m| m.title.clone()), m.and_then(|m| m.url))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    let log_session_id = play_session_id
        .trim_start_matches("audio-")
        .trim_start_matches("video-");
    let position_secs = data.position_ticks.unwrap_or(0) / 10_000_000;
    // For transcode sessions, master_hls_video fires the info log once it has
    // full codec/bitrate/reasons info. For direct play/stream, log here.
    let is_transcode = matches!(data.play_method, Some(api::PlayMethod::Transcode));
    if !is_transcode {
        info!(
            play_session_id = log_session_id,
            %item_id,
            title = %media_title,
            source = ?source_title,
            path = ?source_path,
            user = %session.user.username,
            client = %session.device.app_name,
            play_method = ?data.play_method,
            audio_stream = ?data.audio_stream_index,
            subtitle_stream = ?data.subtitle_stream_index,
            position_secs,
            "▶ Playback started"
        );
    }

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
        let sessions: Vec<crate::api::SessionInfoDto> = resp.json();
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
        let sessions: Vec<crate::api::SessionInfoDto> = resp.json();
        assert_eq!(sessions.len(), 1);
        // id is the device id from the auth header, not the play session id
        assert_eq!(sessions[0].id, Some("test-device".to_string()));
        // now_playing_item is populated for the active playback session
        assert!(sessions[0].now_playing_item.is_some());
    }

    #[tokio::test]
    async fn test_get_sessions_refreshes_device_metadata_from_auth_header() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = format!(
            "MediaBrowser Client=\"Jellyfin Web\", Device=\"Chrome Laptop\", DeviceId=\"test-device\", Version=\"10.11.0\", Token=\"{}\"",
            token
        );

        let resp = server
            .get("/sessions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status_ok();
        let sessions: Vec<crate::api::SessionInfoDto> = resp.json();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].device_name.as_deref(), Some("Chrome Laptop"));
        assert_eq!(sessions[0].client.as_deref(), Some("Jellyfin Web"));
        assert_eq!(sessions[0].application_version.as_deref(), Some("10.11.0"));
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

    /// Without a device profile the server defaults to direct play.
    #[tokio::test]
    async fn test_playbackinfo_no_profile_returns_direct_play() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;

        resp.assert_status_ok();
        // When no MediaSourceId is given, the first source Id must equal the item id
        // so Android TV and other clients can resolve the stream from the path parameter.
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "Id": media.id.to_string(),
                "SupportsTranscoding": true,
                "SupportsDirectPlay": true,
            }]
        }));
    }

    #[tokio::test]
    async fn test_playbackinfo_minimal() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

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
                "Container": "mp4",
                "RunTimeTicks": 100000000,
                "SupportsDirectPlay": true,
                "SupportsTranscoding": true
            }]
        }));
        // Sanity-check bitrate is probed and non-zero (exact value varies by probe).
        let body: serde_json::Value = resp.json();
        assert!(body["MediaSources"][0]["Bitrate"].as_i64().unwrap_or(0) > 0);
    }

    /// A device profile that supports direct play causes the endpoint to return a direct-play response.
    #[tokio::test]
    async fn test_playbackinfo_direct_play_profile() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

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
        // SupportsTranscoding is always true so the client can re-request for subtitle burn-in.
        // No MediaSourceId in request → source Id must equal the item id.
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "Id": media.id.to_string(),
                "SupportsDirectPlay": true,
                "SupportsTranscoding": true,
            }]
        }));
    }

    /// When `MaxStreamingBitrate` is present the transcoding URL must include it
    /// so the HLS handler can cap the video bitrate accordingly.
    #[tokio::test]
    async fn test_playbackinfo_max_streaming_bitrate_in_url() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;
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
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

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
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

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
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

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

    /// When MediaSourceId in the request body equals the item id (Android TV auto-play pattern),
    /// source[0].Id must still equal the item id — not the internal source UUID.
    #[tokio::test]
    async fn test_playbackinfo_media_source_id_equals_item_id() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            // Android TV sends MediaSourceId == the item id for auto-play
            .json(&json!({ "MediaSourceId": media.id.to_string() }))
            .await;

        resp.assert_status_ok();
        resp.assert_json_contains(&json!({
            "MediaSources": [{
                "Id": media.id.to_string(),
            }]
        }));
    }

    /// When a real specific MediaSourceId is provided (different from the item id),
    /// source[0].Id must equal that source's UUID — not the item id — so the client
    /// can send it back on subsequent requests to resolve the correct stream.
    #[tokio::test]
    async fn test_playbackinfo_specific_media_source_id_preserved() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        // insert_test_source creates a Stream which is already its own source
        let source = insert_test_source(&guard.0).await;

        // Request using just the item id (no MediaSourceId) — source Id will be item id.
        let resp_no_sid = server
            .post(&format!("/items/{}/playbackinfo", source.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;
        resp_no_sid.assert_status_ok();
        let body: serde_json::Value = resp_no_sid.json();
        // Without MediaSourceId the server overrides source[0].Id to the item id.
        assert_eq!(
            body["MediaSources"][0]["Id"].as_str().unwrap(),
            source.id.to_string(),
            "source Id should equal item id when no MediaSourceId given"
        );

        // Now request with MediaSourceId == item id (Android TV pattern) — same result.
        let resp_with_sid = server
            .post(&format!("/items/{}/playbackinfo", source.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MediaSourceId": source.id.to_string() }))
            .await;
        resp_with_sid.assert_status_ok();
        let body2: serde_json::Value = resp_with_sid.json();
        assert_eq!(
            body2["MediaSources"][0]["Id"].as_str().unwrap(),
            source.id.to_string(),
            "source Id must equal item id when MediaSourceId == item id (Android TV)"
        );
    }
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackProgressInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        let ps_snapshot = state.ctx.sessions.get(psid);
        if let Some(ref ps) = ps_snapshot {
            let item_id = data.item_id.unwrap_or(ps.item_id);

            // Detect encode-parameter changes and log them once.
            // We ignore pause/unpause — those are not encode changes.
            let audio_changed = data.audio_stream_index.is_some()
                && data.audio_stream_index != ps.audio_stream_index;
            let subtitle_changed = data.subtitle_stream_index.is_some()
                && data.subtitle_stream_index != ps.subtitle_stream_index;
            let method_changed = data.play_method.is_some()
                && data.play_method.as_ref().map(|m| m.to_string()) != ps.play_method;
            if audio_changed || subtitle_changed || method_changed {
                info!(
                    play_session_id = psid.trim_start_matches("audio-").trim_start_matches("video-"),
                    item_id = %item_id,
                    user = %session.user.username,
                    audio_stream = if audio_changed {
                        format!("{:?} → {:?}", ps.audio_stream_index, data.audio_stream_index)
                    } else {
                        format!("{:?}", ps.audio_stream_index)
                    },
                    subtitle_stream = if subtitle_changed {
                        format!("{:?} → {:?}", ps.subtitle_stream_index, data.subtitle_stream_index)
                    } else {
                        format!("{:?}", ps.subtitle_stream_index)
                    },
                    play_method = if method_changed {
                        format!("{:?} → {:?}", ps.play_method, data.play_method)
                    } else {
                        format!("{:?}", ps.play_method)
                    },
                    "⟳ Playback params changed"
                );
            }

            state.ctx.sessions.update(psid, |ps| {
                ps.position_ticks = data.position_ticks.unwrap_or(ps.position_ticks);
                if data.is_paused && !ps.is_paused {
                    ps.last_paused_at = Some(Utc::now());
                } else if !data.is_paused {
                    ps.last_paused_at = None;
                }
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
    Json(data): Json<api::PlaybackStopInfo>,
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

        debug!(play_session_id = psid, "Playback stopped");
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
    State(state): State<AppState>,
    session: auth::AuthSession,
    body: Option<Json<api::ClientCapabilitiesDto>>,
) -> Result<StatusCode> {
    if let Some(Json(caps)) = body {
        let _ =
            auth::Device::save_capabilities(&state.ctx.db, &session.device.id, &caps)
                .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/{id}/capabilities/full")]
pub async fn sessions_capabilities_full_by_id(
    State(state): State<AppState>,
    Path(id): Path<String>,
    _session: auth::AuthSession,
    body: Option<Json<api::ClientCapabilitiesDto>>,
) -> Result<StatusCode> {
    if let Some(Json(caps)) = body {
        let _ = auth::Device::save_capabilities(&state.ctx.db, &id, &caps).await;
    }
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
                device.last_activity_at.is_some_and(|t| t >= cutoff)
            } else {
                true
            }
        })
        .collect();

    let mut sessions = Vec::with_capacity(filtered_devices.len());
    for device in filtered_devices {
        let ps = playback_sessions.iter().find(|s| s.device_id == device.id);

        // Load full media from DB if there's an active playback session.
        let mut media = if let Some(ps) = ps {
            db::Media::get_by_id(&state.ctx.db, &ps.item_id)
                .await
                .ok()
                .flatten()
        } else {
            None
        };

        // Load the source being played directly by media_source_id.
        // The source has probe_data with MediaStreams from ffprobe.
        let mut source_media =
            if let Some(msid) = ps.and_then(|p| p.media_source_id.as_ref()) {
                if let Ok(source_id) = msid.parse::<Uuid>() {
                    db::Media::get_by_id(&state.ctx.db, &source_id)
                        .await
                        .ok()
                        .flatten()
                } else {
                    None
                }
            } else {
                None
            };

        // Fallback: if media_source_id didn't yield probe data, try the first
        // Source child of the item (covers cases where media_source_id is missing
        // or points to the parent item itself without probe data).
        if source_media
            .as_ref()
            .and_then(|m| m.probe_data.as_ref())
            .is_none()
        {
            if let Some(m) = media.as_mut() {
                if let Ok(sources) = m.streams(&state.ctx.db).await {
                    if let Some(s) =
                        sources.into_iter().find(|s| s.probe_data.is_some())
                    {
                        source_media = Some(s);
                    }
                }
            }
        }

        let probe_data = source_media
            .as_ref()
            .and_then(|m| m.probe_data.as_ref())
            .or_else(|| media.as_ref().and_then(|m| m.probe_data.as_ref()));

        // Populate now-playing item from DB for full metadata.
        let now_playing = if let Some(ps) = ps {
            let mut item = media
                .as_ref()
                .map(|m| api::db_media_to_item(m.clone()))
                .unwrap_or_else(|| api::BaseItemDto {
                    id: ps.item_id,
                    ..Default::default()
                });
            // Attach MediaStreams from probe data so clients can see track info.
            if item.media_streams.is_none() {
                if let Some(probe) = probe_data {
                    if !probe.media_streams.is_empty() {
                        item.media_streams = Some(probe.media_streams.clone());
                    }
                }
            }
            Some(item)
        } else {
            None
        };

        // Attach TranscodingInfo with enriched metadata from probe data.
        let transcoding_info = ps
            .and_then(|ps| ps.transcode.as_ref().and_then(|ts| ts.try_read().ok()))
            .map(|ts| {
                // Pull width/height/bitrate/channels from source media probe data.
                let video_stream = probe_data.and_then(|p| {
                    p.media_streams
                        .iter()
                        .find(|s| s.type_ == Some(api::MediaStreamType::Video))
                });
                let width = video_stream.and_then(|v| v.width.map(|x| x as i32));
                let height = video_stream.and_then(|v| v.height.map(|x| x as i32));
                let audio_channels = probe_data.and_then(|p| {
                    p.media_streams
                        .iter()
                        .find(|s| s.type_ == Some(api::MediaStreamType::Audio))
                        .and_then(|a| a.channels.map(|x| x as i32))
                });

                // Compute completion percentage from transcode progress.
                let completion_percentage = if ts.runtime_ticks > 0 {
                    let start_ticks = ts.start_time_secs as i64 * 10_000_000;
                    let last_seg = ts
                        .last_segment_index
                        .load(std::sync::atomic::Ordering::Relaxed)
                        as i64;
                    let transcoded_ticks = start_ticks
                        + (last_seg + 1) * ts.segment_length as i64 * 10_000_000;
                    Some(
                        (transcoded_ticks as f64 / ts.runtime_ticks as f64 * 100.0)
                            .min(100.0),
                    )
                } else {
                    None
                };

                api::TranscodingInfo {
                    audio_codec: Some(ts.audio_codec.clone()),
                    video_codec: Some(ts.video_codec.clone()),
                    container: Some("ts".to_string()),
                    is_video_direct: ts.video_codec == "copy",
                    is_audio_direct: ts.audio_codec == "copy",
                    bitrate: probe_data.and_then(|p| p.bitrate),
                    width,
                    height,
                    audio_channels,
                    completion_percentage,
                    transcode_reasons: ts.transcode_reasons.clone(),
                    ..Default::default()
                }
            });

        // Build PlayState from active playback session.
        let play_state = ps.map(|ps| api::PlayerStateInfo {
            position_ticks: Some(ps.position_ticks),
            can_seek: ps.can_seek,
            is_paused: ps.is_paused,
            is_muted: ps.is_muted,
            volume_level: ps.volume_level,
            audio_stream_index: ps.audio_stream_index,
            subtitle_stream_index: ps.subtitle_stream_index,
            media_source_id: ps.media_source_id.clone(),
            play_method: ps.play_method.clone(),
            repeat_mode: "RepeatNone".to_string(),
            playback_order: "Default".to_string(),
        });

        let capabilities = device.parsed_capabilities();

        let (
            playable_media_types,
            supported_commands,
            supports_media_control,
            supports_remote_control,
        ) = capabilities
            .as_ref()
            .map_or((vec![], vec![], false, false), |c| {
                (
                    c.playable_media_types.clone(),
                    c.supported_commands.clone(),
                    c.supports_media_control,
                    c.supports_media_control,
                )
            });

        let last_paused_date = ps.and_then(|ps| ps.last_paused_at);
        let now_playing_queue: Vec<_> = ps
            .and_then(|ps| ps.now_playing_queue.clone())
            .unwrap_or_default();
        let playlist_item_id = ps.and_then(|ps| ps.playlist_item_id.clone());

        // Populate NowPlayingQueueFullItems from queue item IDs.
        let mut now_playing_queue_full_items =
            Vec::with_capacity(now_playing_queue.len());
        for qi in &now_playing_queue {
            if let Ok(Some(m)) = db::Media::get_by_id(&state.ctx.db, &qi.id).await {
                now_playing_queue_full_items.push(api::db_media_to_item(m));
            }
        }

        let remote_end_point = device.remote_ip.clone();

        let user_name = device
            .user(&state.ctx.db)
            .await?
            .map(|u| u.username)
            .unwrap_or_default();

        sessions.push(api::SessionInfoDto {
            id: Some(device.id.clone()),
            device_id: Some(device.id.clone()),
            device_name: Some(device.name.clone()),
            client: Some(device.app_name.clone()),
            application_version: Some(device.app_version.clone()),
            user_id: device.user_id.to_string(),
            user_name: Some(user_name),
            last_activity_date: device.last_activity_at.unwrap_or_else(Utc::now),
            last_playback_check_in: device.last_activity_at.unwrap_or_else(Utc::now),
            last_paused_date,
            remote_end_point,
            now_playing_item: now_playing,
            now_playing_queue,
            now_playing_queue_full_items,
            playlist_item_id,
            transcoding_info,
            play_state,
            capabilities,
            playable_media_types,
            supported_commands,
            supports_media_control,
            supports_remote_control,
            is_active: true,
            server_id: crate::common::server_id(),
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
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
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
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

/// Jellyfin-compatible master HLS playlist endpoint.
/// Creates a transcode session and returns a master.m3u8 playlist.
#[get("/videos/{id}/master.m3u8")]
pub async fn master_hls_video(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    debug!("master_hls_video: item_id={}, q={:?}", id, q);

    // Add debugging info for crash diagnosis
    tracing::debug!(
        "Starting HLS session setup for item {} with session ID: {:?}",
        id,
        q.play_session_id
    );

    let play_session_id = q
        .play_session_id
        .unwrap_or_else(|| common::get_uuid().as_simple().to_string());

    tracing::debug!("Using play session ID: {}", play_session_id);

    let video_codec = q.video_codec.unwrap_or_else(|| "copy".to_string());
    let audio_codec = q.audio_codec.unwrap_or_else(|| "aac".to_string());
    let segment_length = q.segment_length.unwrap_or(6) as u32;

    // Look up existing session or create a new one.
    // When the client seeks it sends the same PlaySessionId but with a new
    // StartTimeTicks.  In that case we must stop the old transcode job and
    // restart from the requested position — otherwise the player waits for
    // segments that the old job will never produce at the new offset.
    let is_seeking = q.start_time_ticks.is_some_and(|t| t > 0);
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
        if matches!(
            resolved_media.kind,
            db::MediaKind::Movie | db::MediaKind::Episode
        ) {
            let sources = resolved_media.streams(&state.ctx.db).await?;
            resolved_media = if let Some(wanted) = q.media_source_id {
                sources.iter().find(|s| s.id == wanted).cloned()
            } else {
                None
            }
            .or_else(|| sources.into_iter().next())
            .context_not_found("not found", "no playable source found")?;
        } else if resolved_media.kind == db::MediaKind::Track {
            let sources = resolved_media.streams(&state.ctx.db).await?;
            resolved_media = sources
                .into_iter()
                .next()
                .context_not_found("not found", "no stream found for track")?;
        }

        let input_url = resolved_media
            .url
            .as_ref()
            .map(|d| d.server_input(resolved_media.id, state.ctx.config.port))
            .context_not_found("no url", "media source has no URL")?;

        let output_dir =
            std::path::PathBuf::from("transcode_sessions").join(&play_session_id);
        // Keep the API stable (no RunId in URLs) by reusing one on-disk path per
        // PlaySessionId and clearing stale segments when a transcode restarts.
        let _ = std::fs::remove_dir_all(&output_dir);
        let is_live = resolved_media.kind == db::MediaKind::TvChannel;

        // Live streams have no fixed duration — skip all runtime lookups.
        let runtime_ticks = if is_live {
            0
        } else {
            // Try: resolved source runtime → parent item runtime → cached probe data → parent item DB lookup.
            let rt = resolved_media
                .runtime
                .or(media.runtime)
                .filter(|&r| r > 0)
                .map(|r| r * 10_000_000)
                .or_else(|| {
                    resolved_media
                        .probe_data
                        .as_ref()
                        .and_then(|p| p.run_time_ticks)
                });
            match rt {
                Some(t) if t > 0 => t,
                _ => db::Media::get_by_id(&state.ctx.db, &id)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|m| m.runtime)
                    .filter(|&r| r > 0)
                    .map(|r| r * 10_000_000)
                    .unwrap_or(0),
            }
        };
        debug!(runtime_ticks, is_live, segment_length, "transcode session");
        let source_video_stream = resolved_media
            .probe_data
            .as_ref()
            .and_then(|p| p.video_stream());
        let source_video_codec =
            source_video_stream.as_ref().and_then(|s| s.codec.clone());
        let source_video_profile =
            source_video_stream.as_ref().and_then(|s| s.profile.clone());
        let source_video_level = source_video_stream.as_ref().and_then(|s| s.level);
        let source_video_range_type = source_video_stream
            .as_ref()
            .and_then(|s| s.video_range_type);
        let source_video_width = source_video_stream.as_ref().and_then(|s| s.width);
        let source_video_height = source_video_stream.as_ref().and_then(|s| s.height);
        let source_frame_rate =
            source_video_stream.as_ref().and_then(|s| s.real_frame_rate);
        debug!(
            ?source_video_codec,
            ?source_video_profile,
            ?source_video_level,
            ?source_video_range_type,
            source_video_width,
            source_video_height,
            source_frame_rate,
            "source video codec for HLS session"
        );
        let session = TranscodeSession::new(
            play_session_id.clone(),
            id,
            media_source_id,
            input_url.clone(),
            output_dir,
            video_codec.clone(),
            audio_codec.clone(),
            q.audio_stream_index.map(|v| v as i32),
            q.subtitle_stream_index.map(|v| v as i32),
            q.subtitle_method == Some(api::SubtitleDeliveryMethod::Encode),
            segment_length,
            // Parse reasons from query param (set by playbackinfo on the transcoding URL)
            q.transcode_reasons
                .as_deref()
                .map(api::TranscodeReasons::from_query_value)
                .unwrap_or_default(),
            runtime_ticks,
            is_live,
            source_video_codec,
            source_video_profile,
            source_video_level,
            source_video_range_type,
            source_video_width,
            source_video_height,
            source_frame_rate,
        );

        state
            .ctx
            .sessions
            .attach_transcode(&play_session_id, session.clone());

        // Start transcoding in background
        let session_clone = session.clone();
        let encoding_opts = crate::db::Settings::get_encoding_config(&state.ctx.db)
            .await
            .unwrap_or_default();
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
            burn_subtitle: q.subtitle_method
                == Some(api::SubtitleDeliveryMethod::Encode),
            subtitle_width: None,
            subtitle_height: None,
            encoding_preset: encoding_opts.encoding_preset,
            source_video_codec: session.read().await.source_video_codec.clone(),
            hardware_acceleration_type: encoding_opts
                .hardware_acceleration_type
                .unwrap_or_default(),
            vaapi_device: encoding_opts
                .vaapi_device
                .unwrap_or_else(|| "/dev/dri/renderD128".to_string()),
        };

        // Spawn the transcode task with proper error handling
        let media_title_for_log = resolved_media.title.clone();
        let transcode_reasons_for_log = q.transcode_reasons.clone();
        // Pull user/client from the PlaybackSession created by report_playback_start
        let ps_for_log = state.ctx.sessions.get(&play_session_id);
        let session_clone = session.clone();
        tokio::spawn(async move {
            let start_secs = params.start_time_ticks.unwrap_or(0) / 10_000_000;
            let resolution = match (params.max_width, params.max_height) {
                (Some(w), Some(h)) => format!("{}x{}", w, h),
                (Some(w), None) => format!("{}w", w),
                (None, Some(h)) => format!("{}h", h),
                _ => "native".to_string(),
            };
            info!(
                play_session_id = %play_session_id,
                title = %media_title_for_log,
                user = ps_for_log.as_ref().map(|ps| ps.user_id.to_string()).unwrap_or_default(),
                client = ps_for_log.as_ref().map(|ps| ps.client_name.as_str()).unwrap_or(""),
                video_codec = %params.video_codec,
                audio_codec = %params.audio_codec,
                resolution,
                video_bitrate = ?params.video_bitrate,
                hw_accel = ?params.hardware_acceleration_type,
                transcode_reasons = ?transcode_reasons_for_log,
                start_secs,
                "▶ Playback started (transcode)"
            );
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
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    variant_hls_video_inner(state, q).await
}

/// Serves the variant (child) HLS playlist generated by the transcoding engine.
#[get("/videos/{id}/main/stream.m3u8")]
pub async fn variant_hls_video(
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    variant_hls_video_inner(state, q).await
}

async fn variant_hls_video_inner(
    state: AppState,
    q: api::HlsVideoQuery,
) -> Result<impl IntoResponse> {
    let play_session_id = q
        .play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    let session = state
        .ctx
        .sessions
        .get_transcode(&play_session_id)
        .context_not_found("not found", "transcode session not found")?;

    // Keep the session alive.
    state.ctx.sessions.ping(&play_session_id);

    let session_read = session.read().await;
    let is_live = session_read.is_live;
    let use_fmp4 = session_read.use_fmp4();
    let playlist_path = session_read.variant_playlist_path();
    let psid = session_read.id.clone();

    // For live streams and fMP4 sessions we must serve the ffmpeg-written playlist
    // because fMP4 segments snap to keyframe boundaries — actual durations differ
    // from the target, so a synthetic uniform playlist would violate the HLS spec
    // (#EXT-X-TARGETDURATION and #EXTINF must reflect real segment durations).
    if is_live || use_fmp4 {
        drop(session_read);
        // For live streams, serve the ffmpeg-written EVENT playlist directly.
        // For fMP4 VOD, also use ffmpeg's playlist because fMP4 segments snap to
        // keyframe boundaries so actual durations differ from our 6s target.
        // Poll until ffmpeg has written at least the first segment entry.
        let content = tokio::time::timeout(std::time::Duration::from_secs(15), async {
            loop {
                if let Ok(text) = tokio::fs::read_to_string(&playlist_path).await {
                    if text.contains("#EXTINF") {
                        return text;
                    }
                }
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        })
        .await
        .unwrap_or_default();

        // Inject ?PlaySessionId=... into segment/map lines so hls_segment_inner can find the session.
        let content = content
            .lines()
            .map(|line| {
                if !line.starts_with('#')
                    && (line.ends_with(".ts") || line.ends_with(".m4s"))
                {
                    format!("{}?PlaySessionId={}", line, psid)
                } else if line.starts_with("#EXT-X-MAP:")
                    && !line.contains("PlaySessionId")
                {
                    // Inject PlaySessionId into the fMP4 init segment URI.
                    // e.g. #EXT-X-MAP:URI="init.mp4" → #EXT-X-MAP:URI="init.mp4?PlaySessionId=…"
                    line.replace(
                        "\"init.mp4\"",
                        &format!("\"init.mp4?PlaySessionId={}\"", psid),
                    )
                } else {
                    line.to_string()
                }
            })
            .collect::<Vec<_>>()
            .join("\n");

        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "application/vnd.apple.mpegurl")
            .header("Cache-Control", "no-cache, no-store")
            .body(Body::from(content))
            .unwrap());
    }

    debug!(
        runtime_ticks = session_read.runtime_ticks,
        segment_length = session_read.segment_length,
        play_session_id = %play_session_id,
        "Generating VOD variant playlist"
    );
    let content = crate::transcode::engine::generate_variant_playlist(
        &session_read,
        "", // no extra query string needed
    );

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
    Query(q): Query<api::HlsVideoQuery>,
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
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let segment_id = strip_segment_extension(&segment_file);
    hls_segment_inner(state, segment_id, q).await
}

/// Jellyfin-compatible HLS segment route: /Videos/{id}/hls1/{playlistId}/{segmentFile}
#[get("/videos/{id}/hls1/{playlist_id}/{segment_file}")]
pub async fn hls1_segment(
    State(state): State<AppState>,
    Path((id, _playlist_id, segment_file)): Path<(Uuid, String, String)>,
    Query(q): Query<api::HlsVideoQuery>,
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

/// Find the highest segment index currently on disk in `dir`.
fn get_current_transcoding_index(dir: &std::path::Path) -> Option<u32> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut max_idx: Option<u32> = None;
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // Accept both MPEG-TS (.ts) and fMP4 (.m4s) segment files.
        if let Some(idx_str) = name
            .strip_suffix(".ts")
            .or_else(|| name.strip_suffix(".m4s"))
            .and_then(|s| s.rsplit('_').next())
        {
            if let Ok(idx) = idx_str.parse::<u32>() {
                max_idx = Some(max_idx.map_or(idx, |m: u32| m.max(idx)));
            }
        }
    }
    max_idx
}

async fn hls_segment_inner(
    state: AppState,
    segment_id: String,
    q: api::HlsVideoQuery,
) -> Result<impl IntoResponse> {
    let play_session_id = q
        .play_session_id
        .context_not_found("missing", "PlaySessionId is required")?;

    trace!(
        segment_id = %segment_id,
        play_session_id = %play_session_id,
        runtime_ticks = ?q.runtime_ticks,
        "HLS segment request"
    );

    let session = state.ctx.sessions.get_transcode(&play_session_id);

    // The fMP4 init segment is served at "init.mp4" — strip_segment_extension
    // reduces that to "init", so we detect it here and serve it directly.
    if segment_id == "init" {
        let init_path = match &session {
            Some(s) => s.read().await.init_segment_path(),
            None => state
                .ctx
                .sessions
                .segment_path(&play_session_id, "init.mp4")
                .with_extension("mp4"),
        };
        // Wait briefly for ffmpeg to write the init segment.
        if session.is_some() {
            let mut attempts = 0;
            while !init_path.exists() && attempts < 40 {
                tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                attempts += 1;
            }
        }
        if !init_path.exists() {
            None::<()>.context_not_found("not found", "fMP4 init segment not ready")?;
        }
        state.ctx.sessions.ping(&play_session_id);
        let file = tokio::fs::File::open(&init_path).await?;
        let stream = ReaderStream::new(file);
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", "video/mp4")
            .header("Cache-Control", "public, max-age=86400")
            .body(Body::from_stream(stream))
            .unwrap());
    }

    // Derive the segment path — either from the live session or from the base
    // dir directly (handles server restart where session is gone but files remain).
    let segment_path = match &session {
        Some(s) => s.read().await.segment_path(&segment_id),
        None => state
            .ctx
            .sessions
            .segment_path(&play_session_id, &segment_id),
    };

    // Parse the requested segment index from the filename.
    let requested_idx: Option<u32> = segment_id
        .rsplit('_')
        .next()
        .and_then(|n| n.parse::<u32>().ok());

    if let Some(ref session) = session {
        // Update playback position for the buffer monitor.
        if let Some(idx) = requested_idx {
            use std::sync::atomic::Ordering;
            let s = session.read().await;
            let prev = s.last_segment_index.load(Ordering::Relaxed);
            if idx > prev {
                s.last_segment_index.store(idx, Ordering::Relaxed);
            }
        }
    }

    // If the segment doesn't exist and we have a live session, check whether
    // FFmpeg needs to be restarted at a different position (like Jellyfin does).
    if !segment_path.exists() {
        if let (Some(session), Some(requested_idx)) = (&session, requested_idx) {
            let s = session.read().await;
            let output_dir = s.output_dir.clone();
            let segment_length = s.segment_length;
            let current_idx = get_current_transcoding_index(&output_dir);
            let segment_gap_threshold = 24 / segment_length;

            let needs_restart = match current_idx {
                None => {
                    // No segments on disk yet. If FFmpeg is still running
                    // (Starting/Running), just fall through to the wait loop —
                    // killing it here causes an infinite restart cycle.
                    matches!(
                        s.state,
                        TranscodeState::Error(_) | TranscodeState::Complete
                    )
                }
                Some(cur) if requested_idx < cur => true, // seeking backward
                Some(cur)
                    if requested_idx.saturating_sub(cur) > segment_gap_threshold =>
                {
                    true
                } // too far ahead
                _ => false, // within range — just wait for FFmpeg
            };

            if needs_restart {
                // Guard against concurrent restart: only proceed if FFmpeg
                // is actually running (kill_tx is Some). If another request
                // already killed it and started a new one, just wait.
                let has_running_ffmpeg = s.kill_tx.is_some();
                if !has_running_ffmpeg {
                    drop(s);
                    // Another request already restarted — fall through to wait loop.
                } else {
                    debug!(
                        requested_idx,
                        ?current_idx,
                        segment_gap_threshold,
                        "Segment-driven transcode restart"
                    );

                    // Gather params we need before dropping the read lock.
                    let input_url = s.input_url.clone();
                    let video_codec = s.video_codec.clone();
                    let audio_codec = s.audio_codec.clone();
                    let audio_stream_index = s.audio_stream_index;
                    let subtitle_stream_index = s.subtitle_stream_index;
                    let burn_subtitle = s.burn_subtitle;
                    drop(s);

                    // Kill running FFmpeg and clean up stale segments (params
                    // like bitrate/codec may change, so old segments are invalid).
                    {
                        let (kill_tx, wait_done) = {
                            let mut s = session.write().await;
                            (s.kill_tx.take(), s.wait_done.clone())
                        };
                        if let Some(kill_tx) = kill_tx {
                            let notification = wait_done.notified();
                            let _ = kill_tx.send(());
                            notification.await;
                        }
                    }
                    let _ = std::fs::remove_dir_all(&output_dir);
                    let _ = std::fs::create_dir_all(&output_dir);

                    // Calculate the seek position from the runtimeTicks query param
                    // (cumulative ticks to start of this segment) provided by our
                    // server-generated VOD playlist. Fall back to segment_index * segment_length.
                    let start_time_ticks = q.runtime_ticks.unwrap_or_else(|| {
                        requested_idx as i64 * segment_length as i64 * 10_000_000
                    });

                    let encoding_opts =
                        crate::db::Settings::get_encoding_config(&state.ctx.db)
                            .await
                            .unwrap_or_default();
                    let params = crate::transcode::engine::TranscodeParams {
                        input_url,
                        output_dir: output_dir.clone(),
                        video_codec,
                        audio_codec: audio_codec.clone(),
                        segment_length,
                        start_time_ticks: Some(start_time_ticks),
                        max_width: q.max_width.map(|v| v as u32),
                        max_height: q.max_height.map(|v| v as u32),
                        video_bitrate: q
                            .video_bit_rate
                            .map(|v| v as u32)
                            .or_else(|| q.max_streaming_bitrate.map(|b| b as u32)),
                        audio_bitrate: q.audio_bit_rate.map(|v| v as u32),
                        audio_channels: if audio_codec == "copy" {
                            None
                        } else {
                            Some(2)
                        },
                        audio_stream_index,
                        subtitle_stream_index,
                        burn_subtitle,
                        subtitle_width: None,
                        subtitle_height: None,
                        encoding_preset: encoding_opts.encoding_preset,
                        source_video_codec: session
                            .read()
                            .await
                            .source_video_codec
                            .clone(),
                        hardware_acceleration_type: encoding_opts
                            .hardware_acceleration_type
                            .unwrap_or_default(),
                        vaapi_device: encoding_opts
                            .vaapi_device
                            .unwrap_or_else(|| "/dev/dri/renderD128".to_string()),
                    };

                    // Reinitialise the session's state for the new transcode run.
                    {
                        let mut s = session.write().await;
                        s.state = TranscodeState::Starting;
                        let _ = s.state_tx.send(TranscodeState::Starting);
                        s.start_time_secs = (start_time_ticks / 10_000_000) as u32;
                        s.playback_offset_secs.store(
                            s.start_time_secs,
                            std::sync::atomic::Ordering::Relaxed,
                        );
                    }

                    let session_clone = session.clone();
                    tokio::spawn(async move {
                        if let Err(e) = crate::transcode::engine::start_transcode(
                            session_clone,
                            params,
                        )
                        .await
                        {
                            tracing::error!("Transcode restart failed: {:#}", e);
                        }
                    });
                } // else: has_running_ffmpeg
            } // needs_restart
        }
    }

    // Wait up to 60s for ffmpeg to produce the segment.
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

    // fMP4 segments (.m4s) use video/mp4; MPEG-TS segments use video/mp2t.
    let content_type =
        if segment_path.extension().and_then(|e| e.to_str()) == Some("m4s") {
            "video/mp4"
        } else {
            "video/mp2t"
        };

    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", content_type)
        .header("Cache-Control", "public, max-age=86400")
        .body(body)
        .unwrap())
}

/// Stops and cleans up a transcoding session.
#[delete("/videos/activeencodings")]
pub async fn delete_transcoding(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<api::HlsVideoQuery>,
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
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

/// Audio universal stream endpoint used by Jellyfin mobile/web clients for music tracks.
/// Resolves the track's CDN stream URL via yt-dlp and redirects to it so the server
/// acts as the origin (clients cannot reach IP-locked YouTube CDN URLs directly).
#[get("/audio/{id}/universal")]
pub async fn audio_universal(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(&state.ctx.db, &id)
        .await?
        .context_not_found("not found", "track not found")?;

    let play_session_id = q
        .play_session_id
        .unwrap_or_else(|| common::get_uuid().as_simple().to_string());

    let transcoding_url = format!(
        "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec=copy&AudioCodec=aac&ApiKey={}",
        id, play_session_id, id, session.device.access_token
    );

    Ok(axum::response::Redirect::temporary(&transcoding_url).into_response())
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

/// Subtitle extraction endpoint - extracts a subtitle stream from a media source
/// and optionally converts it to the requested format (vtt, srt, ass).
// Jellyfin clients include a start-position-ticks segment in the path.
#[get(
    "/videos/{item_id}/{media_source_id}/subtitles/{stream_index}/{start_ticks}/stream.{format}"
)]
pub async fn subtitles_stream(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((item_id, media_source_id, stream_index, _start_ticks, format)): Path<(
        Uuid,
        Uuid,
        i64,
        String,
        String,
    )>,
    axum::extract::Query(params): axum::extract::Query<
        std::collections::HashMap<String, String>,
    >,
) -> Result<impl IntoResponse> {
    let _ = item_id;

    // External subtitle proxy: fetch from source URL and convert to requested format.
    if let Some(source_url) = params.get("SubtitleUrl") {
        let source_url = urlencoding::decode(source_url)
            .map(|s| s.into_owned())
            .unwrap_or_else(|_| source_url.clone());
        let output_format = format.to_ascii_lowercase();
        let content_type = match output_format.as_str() {
            "vtt" | "webvtt" => "text/vtt; charset=utf-8",
            _ => "text/plain; charset=utf-8",
        };
        let body = reqwest::get(&source_url)
            .await
            .map_err(|e| anyhow!("failed to fetch external subtitle: {e}"))?
            .text()
            .await
            .map_err(|e| anyhow!("failed to read external subtitle: {e}"))?;
        let converted = if matches!(output_format.as_str(), "vtt" | "webvtt") {
            srt_to_vtt(&body)
        } else {
            body
        };
        return Ok(Response::builder()
            .status(StatusCode::OK)
            .header("Content-Type", content_type)
            .header("Cache-Control", "public, max-age=3600")
            .header("Access-Control-Allow-Origin", "*")
            .body(Body::from(converted))
            .unwrap());
    }

    let mut media = db::Media::get_by_id(&state.ctx.db, &media_source_id)
        .await?
        .context_not_found("not found", "media source not found")?;

    if matches!(
        media.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        media = media
            .streams(&state.ctx.db)
            .await?
            .get(0)
            .context_not_found("not found", "no sources found")?
            .clone();
    }

    let url = media
        .url
        .as_ref()
        .map(|d| d.server_input(media.id, state.ctx.config.port))
        .context_not_found("no url", "media source has no URL")?;

    let output_format = format.to_ascii_lowercase();
    let (ffmpeg_format, content_type) = match output_format.as_str() {
        "vtt" | "webvtt" => ("webvtt", "text/vtt; charset=utf-8"),
        "srt" | "subrip" => ("srt", "text/plain; charset=utf-8"),
        "ass" | "ssa" => ("ass", "text/plain; charset=utf-8"),
        "pgssub" | "sup" => ("sup", "application/octet-stream"),
        _ => ("srt", "text/plain; charset=utf-8"),
    };

    // FFmpeg expects subtitle-ordinal form (0:s:N) for reliable subtitle mapping.
    // Clients pass the Jellyfin stream index, so convert when probe metadata exists.
    let map_spec = media
        .probe_data
        .as_ref()
        .and_then(|probe| {
            let mut sub_indexes: Vec<i64> = probe
                .media_streams
                .iter()
                .filter(|s| matches!(s.type_, Some(api::MediaStreamType::Subtitle)))
                .map(|s| s.index)
                .collect();

            sub_indexes.sort_unstable();
            sub_indexes
                .iter()
                .position(|idx| *idx == stream_index)
                .map(|ordinal| format!("0:s:{}", ordinal))
        })
        .unwrap_or_else(|| format!("0:{stream_index}"));

    let mut cmd = tokio::process::Command::new(ffmpeg_bin());
    cmd.args([
        "-i",
        &url,
        "-map",
        &map_spec,
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
        tracing::error!(
            media_source_id = %media_source_id,
            stream_index,
            map = %map_spec,
            format = %ffmpeg_format,
            "ffmpeg subtitle extraction failed: {stderr}"
        );
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

/// Convert SRT subtitle text to WebVTT. Already-valid VTT is passed through unchanged.
fn srt_to_vtt(input: &str) -> String {
    if input.trim_start().starts_with("WEBVTT") {
        return input.to_string();
    }
    let mut out = String::from("WEBVTT\n\n");
    for block in input.trim().split("\n\n") {
        let lines: Vec<&str> = block.lines().collect();
        if lines.len() < 2 {
            continue;
        }
        // Skip the sequence number line (all digits), keep timecodes + text
        let rest = if lines[0].trim().chars().all(|c| c.is_ascii_digit()) {
            &lines[1..]
        } else {
            &lines[..]
        };
        if rest.is_empty() {
            continue;
        }
        // Convert SRT timestamp separator , → .
        let timecode = rest[0].replace(',', ".");
        out.push_str(&timecode);
        out.push('\n');
        for line in &rest[1..] {
            out.push_str(line);
            out.push('\n');
        }
        out.push('\n');
    }
    out
}

pub(crate) fn lang_to_two_letter(lang: &str) -> Option<String> {
    use std::str::FromStr;
    let lang = lang.trim().to_lowercase();
    if lang.is_empty() {
        return None;
    }
    if lang.len() == 2 {
        return Some(lang);
    }
    isolang::Language::from_639_3(&lang)
        .or_else(|| isolang::Language::from_str(&lang).ok())
        .and_then(|l| l.to_639_1())
        .map(|s| s.to_string())
}

fn score_sub_url(
    sub_url: &str,
    source_name: &Option<String>,
    source_path: &Option<String>,
) -> i32 {
    fn tokens(s: &str) -> std::collections::HashSet<String> {
        s.split(|c: char| !c.is_alphanumeric())
            .filter(|t| t.len() > 2)
            .map(|t| t.to_lowercase())
            .collect()
    }
    let sub_file = sub_url.rsplit('/').next().unwrap_or(sub_url);
    let sub_tok = tokens(sub_file);
    let mut src_tok = tokens(source_name.as_deref().unwrap_or(""));
    src_tok.extend(tokens(source_path.as_deref().unwrap_or("")));
    sub_tok.intersection(&src_tok).count() as i32
}

/// Inject external subtitles into a list of `MediaSourceInfo` entries.
pub(super) async fn inject_external_subtitles(
    ctx: &crate::AppContext,
    subtitle_media: &crate::db::Media,
    media_sources: &mut Vec<api::MediaSourceInfo>,
    item_id: Uuid,
    api_key: &str,
) {
    let subs = ctx.addons.fetch_subtitles(subtitle_media, &ctx.db).await;
    if subs.is_empty() {
        return;
    }

    let sub_langs: Vec<String> = crate::db::Settings::get_config(&ctx.db)
        .await
        .ok()
        .and_then(|c| c.subtitle_languages)
        .unwrap_or_default();

    let filtered: Vec<_> = if sub_langs.is_empty() {
        subs
    } else {
        subs.into_iter()
            .filter(|s| {
                let two = s.lang.as_deref().and_then(lang_to_two_letter);
                two.map_or(false, |two| {
                    sub_langs.iter().any(|p| two.eq_ignore_ascii_case(p.trim()))
                })
            })
            .collect()
    };

    if filtered.is_empty() {
        return;
    }

    use crate::sdks;
    for source in media_sources.iter_mut() {
        let next_idx = source
            .media_streams
            .iter()
            .map(|s| s.index)
            .max()
            .map_or(0, |m| m + 1);

        let mut scored: Vec<_> = filtered
            .iter()
            .map(|s| (score_sub_url(&s.url, &source.name, &source.path), s))
            .collect();
        scored.sort_by(|(sa, a), (sb, b)| {
            let rank = |s: &&sdks::stremio::Subtitle| {
                let two = s.lang.as_deref().and_then(lang_to_two_letter);
                sub_langs
                    .iter()
                    .position(|p| {
                        two.as_deref()
                            .map_or(false, |t| t.eq_ignore_ascii_case(p.trim()))
                    })
                    .unwrap_or(usize::MAX)
            };
            rank(a).cmp(&rank(b)).then(sb.cmp(sa))
        });

        let mut lang_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let scored: Vec<_> = scored
            .into_iter()
            .filter(|(_, s)| {
                let key = s.lang.clone().unwrap_or_else(|| "und".to_string());
                let count = lang_counts.entry(key).or_insert(0);
                if *count < 2 {
                    *count += 1;
                    true
                } else {
                    false
                }
            })
            .collect();

        let wants_default =
            !sub_langs.is_empty() && source.default_subtitle_stream_index.is_none();
        for (i, (_, sub)) in scored.iter().enumerate() {
            let mut stream =
                crate::conversions::subtitle_to_media_stream((*sub).clone());
            let idx = next_idx + i as i64;
            stream.index = idx;
            let encoded_url = urlencoding::encode(&sub.url);
            stream.delivery_url = Some(format!(
                "/Videos/{item_id}/{source_id}/Subtitles/{idx}/0/Stream.vtt?ApiKey={api_key}&SubtitleUrl={encoded_url}",
                source_id = source.id,
            ));
            if wants_default && i == 0 {
                stream.is_default = Some(true);
                source.default_subtitle_stream_index = Some(next_idx);
            }
            source.media_streams.push(stream);
        }
    }
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
    media_sources: &mut Vec<api::MediaSourceInfo>,
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
                    s.index == idx
                        && matches!(s.type_, Some(api::MediaStreamType::Audio))
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
                    s.index == idx
                        && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                });
                if exists {
                    // Clear any previous default flag, set the recalled one
                    for s in source.media_streams.iter_mut() {
                        if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                            s.is_default = Some(false);
                        }
                    }
                    source.default_subtitle_stream_index = Some(idx);
                    if let Some(s) =
                        source.media_streams.iter_mut().find(|s| s.index == idx)
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
                let pref_two = lang_to_two_letter(pref);
                if let Some(ref target) = pref_two {
                    if let Some(stream) = source.media_streams.iter_mut().find(|s| {
                        matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                            && s.language
                                .as_deref()
                                .and_then(lang_to_two_letter)
                                .as_deref()
                                == Some(target.as_str())
                    }) {
                        let idx = stream.index;
                        stream.is_default = Some(true);
                        source.default_subtitle_stream_index = Some(idx);
                    }
                }
            }
        }

        // --- subtitle_mode ---
        apply_subtitle_mode(&cfg.subtitle_mode, source);
    }
}

fn apply_subtitle_mode(mode: &api::SubtitleMode, source: &mut api::MediaSourceInfo) {
    let clear_all = |source: &mut api::MediaSourceInfo| {
        for s in source.media_streams.iter_mut() {
            if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = None;
    };

    let set_default = |source: &mut api::MediaSourceInfo, idx: Option<i64>| {
        for s in source.media_streams.iter_mut() {
            if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = idx;
        if let Some(i) = idx {
            if let Some(s) = source.media_streams.iter_mut().find(|s| s.index == i) {
                s.is_default = Some(true);
            }
        }
    };

    match mode {
        api::SubtitleMode::None => {
            // Never auto-show subtitles
            clear_all(source);
        }
        api::SubtitleMode::Always => {
            // If no subtitle is already selected, pick the first non-forced subtitle
            if source.default_subtitle_stream_index.is_none() {
                let idx = source.media_streams.iter().find_map(|s| {
                    if matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                        && !s.is_forced
                    {
                        Some(s.index)
                    } else {
                        None
                    }
                });
                if idx.is_some() {
                    set_default(source, idx);
                }
            }
        }
        api::SubtitleMode::OnlyForced => {
            // Only a forced subtitle may be default; clear any non-forced default
            let forced_idx = source.media_streams.iter().find_map(|s| {
                if matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                    && s.is_forced
                {
                    Some(s.index)
                } else {
                    None
                }
            });
            // Replace whatever is set with the first forced sub (or nothing)
            set_default(source, forced_idx);
        }
        api::SubtitleMode::Smart => {
            // Like Default but clear the selection if the subtitle language already
            // matches the audio language (i.e. no translation needed).
            if let Some(def_idx) = source.default_subtitle_stream_index {
                let audio_lang = source
                    .media_streams
                    .iter()
                    .find(|s| {
                        matches!(s.type_, Some(api::MediaStreamType::Audio))
                            && Some(s.index) == source.default_audio_stream_index
                    })
                    .and_then(|s| s.language.clone());

                let sub_lang = source
                    .media_streams
                    .iter()
                    .find(|s| s.index == def_idx)
                    .and_then(|s| s.language.clone());

                let audio_two = audio_lang.as_deref().and_then(lang_to_two_letter);
                let sub_two = sub_lang.as_deref().and_then(lang_to_two_letter);

                if audio_two.is_some() && audio_two == sub_two {
                    // Subtitle language matches audio — no need to display it
                    clear_all(source);
                }
            }
        }
        // Default: do not alter what was already set by prior steps
        api::SubtitleMode::Default => {}
    }
}
