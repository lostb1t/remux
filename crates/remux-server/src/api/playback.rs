use anyhow::anyhow;
use axum::Json;

use super::subtitles::{
    inject_external_subtitles, inject_sidecar_subtitles, save_sidecar_subtitle_routes,
    scored_external_subtitles,
};
use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_extra::extract::Query;
use chrono::Utc;
use futures_util::{StreamExt, TryStreamExt};
use headers;
use http::{Response, StatusCode};
use remux_macros::{delete, get, post, query};
use remux_sdks::remux::VideoContainer;
use remux_utils::Store;
use serde::Deserialize;
use serde_json::json;
use serde_with::{DurationSeconds, serde_as};
use std::{io, time::Duration};
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, trace, warn};
use url::Url;
use uuid::Uuid;

use crate::{
    AppState, api,
    api::MediaSourceInfoExt,
    common,
    common::{TickUnit, ToRunTimeTicks},
    db,
    db::auth,
    playback::hw_accel,
};

use crate::{
    IntoApiError, OptionExt, ResultExt,
    device_profile::{DeviceProfileExt, SubtitleCodec, subtitle_codec_matches_profile},
    playback::{
        decision::{
            PlaybackConfig, TranscodeDecision, apply_subtitle_delivery,
            build_transcode_decision,
        },
        session::{TranscodeSession, TranscodeState},
    },
    sdks,
    services::{MediaResolveService, ProbeResult, StreamService, StreamServiceConfig},
    torrent,
};
use axum_anyhow::ApiResult as Result;

#[post("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(query): Query<api::PlaybackInfoQuery>,
    Json(payload): Json<api::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    // Jellyfin web sends MediaSourceId as query param even for POST; merge query and body
    let mut q = payload;
    q.media_source_id = q
        .media_source_id
        .or(query.media_source_id);
    q.user_id = q
        .user_id
        .or(query.user_id);
    q.device_profile = q
        .device_profile
        .or(query.device_profile);
    items_playbackinfo_inner(state, session, id, q).await
}

#[get("/items/{id}/playbackinfo")]
pub async fn items_playbackinfo_get(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::PlaybackInfoQuery>,
) -> Result<impl IntoResponse> {
    items_playbackinfo_inner(state, session, id, q).await
}

/// Load remembered audio/subtitle stream selections for a user+item
/// (best-effort; failure means no recall).
async fn load_saved_selections(
    db: &sqlx::SqlitePool,
    user_id: &uuid::Uuid,
    media_id: &uuid::Uuid,
) -> (Option<i64>, Option<i64>) {
    let media = crate::db::Media::get_by_id(db, media_id)
        .await
        .ok()
        .flatten();
    let Some(media) = media else {
        return (None, None);
    };
    match sqlx::query_as::<_, crate::db::UserMediaState>(
        "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
    )
    .bind(user_id)
    .bind(media.id)
    .fetch_optional(db)
    .await
    {
        Ok(Some(state)) => (state.audio_idx, state.subtitle_idx),
        _ => (None, None),
    }
}

fn apply_item_runtime_fallback(
    source: &mut api::MediaSourceInfo,
    item_runtime_seconds: Option<i64>,
) {
    if source
        .run_time_ticks
        .is_some_and(|ticks| ticks > 0)
    {
        return;
    }

    source.run_time_ticks = item_runtime_seconds
        .filter(|seconds| *seconds > 0)
        .and_then(|seconds| seconds.to_ticks(TickUnit::Seconds));
}

fn playback_original_language(
    item: Option<&db::Media>,
    selected_source_language: Option<String>,
) -> Option<String> {
    item.and_then(|media| {
        media
            .original_language
            .clone()
    })
    .or(selected_source_language)
}

async fn items_playbackinfo_inner(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
    q: api::PlaybackInfoQuery,
) -> Result<impl IntoResponse> {
    let media_source_id = q.media_source_id;

    trace!(?id, ?q, "items_playbackinfo");

    let device_profile = q
        .device_profile
        .clone();

    let probe_cfg = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let show_ungrouped = probe_cfg
        .stream_groups_show_ungrouped
        .unwrap_or(true);
    let encoding_cfg = db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();

    if session
        .user
        .policy
        .as_ref()
        .is_some_and(|p| !p.enable_media_playback)
    {
        return Err(anyhow::anyhow!("Forbidden")
            .context_forbidden("media playback is disabled"));
    }

    let media =
        MediaResolveService::resolve_item(media_source_id.unwrap_or(id), &state.ctx)
            .await?
            .context_not_found("not found")?;

    let mut service = StreamService::new(StreamServiceConfig {
        ctx: state
            .ctx
            .clone(),
        item_id: id,
        requested_id: media_source_id,
        show_ungrouped,
        stream_filter: session
            .user
            .policy
            .as_ref()
            .and_then(|p| {
                p.stream_filter
                    .clone()
            }),
        user_id: Some(
            session
                .user
                .id,
        ),
    });
    let is_live = media.is_live();
    let is_track_item = media.is_track();
    let selected_source_language = media
        .original_language
        .clone();
    service
        .load(media)
        .await?;
    // Load the top-level Movie/Episode for subtitle lookup.
    // `id` is always the movie/episode UUID; `media_source_id` may point to a
    // child Source, so we always resolve via `id` to get the IMDB fields.
    let mut subtitle_media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .ok()
    .flatten();
    let item_runtime_seconds = subtitle_media
        .as_ref()
        .and_then(|item| item.runtime);
    let original_language =
        playback_original_language(subtitle_media.as_ref(), selected_source_language);

    let is_track = is_track_item
        || subtitle_media
            .as_ref()
            .map_or(false, |m| m.is_track());
    let has_lyrics = is_track;

    let max_bitrate: Option<i64> = match (
        q.max_streaming_bitrate,
        device_profile
            .as_ref()
            .and_then(|p| p.max_streaming_bitrate),
    ) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    };

    let play_session_id = common::get_uuid()
        .as_simple()
        .to_string();

    let subtitle_mode = encoding_cfg
        .subtitle_mode
        .unwrap_or_default();
    let cfg = PlaybackConfig {
        encoding_cfg,
        device_profile: device_profile.clone(),
        max_bitrate,
        play_session_id: play_session_id.clone(),
        item_id: id,
        subtitle_mode,
    };

    let port = state
        .ctx
        .config
        .port;
    let probed = service
        .probe_candidates()
        .await?;
    let specific_stream_requested = probed.specific_requested;
    let mut media_sources = Vec::with_capacity(
        probed
            .results
            .len(),
    );
    let mut sidecar_subtitle_routes = Vec::with_capacity(
        probed
            .results
            .len(),
    );

    // Per-user playback preferences + remembered selections, resolved per source
    // via `MediaSourceInfo::resolve_default_streams` (see below).
    let user_cfg = session
        .user
        .configuration
        .as_ref()
        .map(|c| {
            c.0.clone()
        })
        .unwrap_or_default();
    let server_subtitle_lang = probe_cfg
        .preferred_metadata_language
        .as_deref();
    let (saved_audio, saved_subtitle) = load_saved_selections(
        &state
            .ctx
            .db,
        &session
            .user
            .id,
        &id,
    )
    .await;

    for ProbeResult {
        mut source,
        stream,
        effective_stream,
    } in probed.results
    {
        // Metadata-only torrent probes may not know the container duration yet.
        // Keep the authoritative Movie/Episode duration in PlaybackInfo so
        // clients do not treat a normal VOD source as an indefinite stream.
        apply_item_runtime_fallback(&mut source, item_runtime_seconds);

        if has_lyrics {
            api::inject_lyric_stream(&mut source);
        }

        // Only flag bitrate exceeded when the source bitrate is known and
        // actually exceeds the cap. An unknown bitrate is treated as within
        // limits so that clients with a high/unlimited cap aren't forced into
        // transcoding unnecessarily.
        let bitrate_exceeded = max_bitrate.map_or(false, |max| {
            source
                .bitrate
                .map_or(false, |b| b > max)
        });

        let mut transcode_reasons: api::TranscodeReasons = {
            let mut reasons = device_profile
                .as_ref()
                .map(|profile| profile.check_direct_play(&source))
                .unwrap_or_default();
            if bitrate_exceeded {
                reasons.insert(api::TranscodeReason::ContainerBitrateExceedsLimit);
            }
            // RTSP streams can only be served via ffmpeg — never direct-playable.
            if matches!(
                stream
                    .stream_info
                    .as_ref()
                    .map(|si| &si.descriptor),
                Some(crate::stream::StreamDescriptor::Rtsp { .. })
            ) {
                reasons.insert(api::TranscodeReason::ContainerNotSupported(
                    "rtsp".to_string(),
                ));
            }
            reasons
        };

        // Strip mode: remove embedded subtitle streams not supported by the client so
        // they don't trigger a transcode. External/addon subs are never touched.
        if subtitle_mode == remux_sdks::remux::EmbeddedSubtitleHandling::Strip {
            source
                .media_streams
                .retain(|s| {
                    !matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                        || s.is_external
                        || device_profile
                            .as_ref()
                            .map(|dp| {
                                dp.subtitle_profiles
                                    .iter()
                                    .filter_map(|p| {
                                        p.format
                                            .as_deref()
                                    })
                                    .any(|f| {
                                        s.codec
                                            .as_deref()
                                            .map_or(false, |c| {
                                                subtitle_codec_matches_profile(c, f)
                                            })
                                    })
                            })
                            .unwrap_or(true)
                });
        }

        // Pre-extract all embedded text subtitle streams in the background, in one
        // FFmpeg pass. By the time the client requests a subtitle URL, the cache file
        // is already written (same approach Jellyfin uses).
        // Use effective_stream so the URL matches the stream whose track layout was probed.
        let effective_url = effective_stream
            .stream_info
            .as_ref()
            .map(|si| {
                si.descriptor
                    .server_input(effective_stream.id, port)
            });
        let _ = effective_url;

        // Resolve default audio/subtitle stream indexes for this source. These are
        // per-request API values (never persisted); resolving before the burn
        // check, transcode decision and subtitle delivery means those consumers
        // see the stream the client will actually get.
        source.resolve_default_streams(
            &user_cfg,
            server_subtitle_lang,
            original_language.as_deref(),
            q.audio_stream_index,
            q.subtitle_stream_index,
            saved_audio,
            saved_subtitle,
        );

        // Detect embedded subtitle codecs unsupported by the client device profile.
        // In Burn mode this triggers transcoding so the subtitle can be burned in.
        // In Extract/Strip modes, no transcode reason is added for subtitles.
        let effective_sub_idx = q
            .subtitle_stream_index
            .or(source.default_subtitle_stream_index);
        if let Some(idx) = effective_sub_idx {
            let needs_burn = subtitle_mode
                == remux_sdks::remux::EmbeddedSubtitleHandling::Burn
                && source
                    .media_streams
                    .iter()
                    .any(|s| {
                        s.index == idx
                            && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                            && !s.is_external
                            && !s.is_text_subtitle_stream
                            && !device_profile
                                .as_ref()
                                .map(|dp| {
                                    dp.subtitle_profiles
                                        .iter()
                                        .filter_map(|p| {
                                            p.format
                                                .as_deref()
                                        })
                                        .any(|f| {
                                            s.codec
                                                .as_deref()
                                                .map_or(false, |c| {
                                                    subtitle_codec_matches_profile(c, f)
                                                })
                                        })
                                })
                                .unwrap_or(false)
                    });
            if needs_burn {
                let codec = source
                    .media_streams
                    .iter()
                    .find(|s| s.index == idx)
                    .and_then(|s| {
                        s.codec
                            .clone()
                    })
                    .unwrap_or_default();
                transcode_reasons
                    .insert(api::TranscodeReason::SubtitleCodecNotSupported(codec));
            }
        }

        debug!(
            stream_id = %stream.id,
            transcode_reasons = ?transcode_reasons,
            "playback decision"
        );

        match build_transcode_decision(
            &source,
            &transcode_reasons,
            effective_sub_idx,
            &q,
            &session,
            &cfg,
        ) {
            TranscodeDecision::DirectPlay => {
                // Keep transcoding available so clients can re-request with a subtitle
                // index (e.g. PGS burn-in) even when direct-play is otherwise fine.
                source.supports_transcoding = true;
                source.supports_direct_play = true;
            }
            TranscodeDecision::Transcode(outcome) => outcome.apply_to(&mut source),
        }

        let torrent = state
            .ctx
            .torrent
            .read()
            .await
            .clone();
        let sidecars = effective_stream
            .stream_info
            .as_ref()
            .and_then(|stream| {
                torrent
                    .as_ref()
                    .map(|mgr| stream.subtitle_sidecars(mgr))
            })
            .unwrap_or_default();
        let routes = inject_sidecar_subtitles(&mut source, sidecars);
        let subtitle_source_id = source.id;

        apply_subtitle_delivery(
            &mut source,
            id,
            session
                .device
                .access_token
                .expose(),
            &cfg.device_profile,
            cfg.subtitle_mode,
        );

        source.transcoding_reasons = transcode_reasons;

        // Recompute from codec — never trust the stored DB value (may be stale).
        for s in &mut source.media_streams {
            if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                s.is_text_subtitle_stream = s.is_text_subtitle_stream();
            }
        }

        sidecar_subtitle_routes.push((subtitle_source_id, routes));
        media_sources.push(source);
    }

    // Inject external subtitles from AIO (cache-backed)
    if let Some(ref mut sub_media) = subtitle_media {
        let sub_langs = probe_cfg
            .subtitle_languages
            .clone()
            .unwrap_or_default();
        inject_external_subtitles(
            &state.ctx,
            sub_media,
            &mut media_sources,
            id,
            session
                .device
                .access_token
                .expose(),
            sub_langs,
            Some(
                session
                    .user
                    .id,
            ),
        )
        .await;
    }

    // Re-resolve defaults after external subtitles were injected so language
    // matching can also pick addon subtitles (same request context as the
    // per-source resolve inside the probe loop).
    for source in &mut media_sources {
        source.resolve_default_streams(
            &user_cfg,
            server_subtitle_lang,
            original_language.as_deref(),
            q.audio_stream_index,
            q.subtitle_stream_index,
            saved_audio,
            saved_subtitle,
        );
    }

    // Cache the group-resolved stream UUID so the stream endpoint can find it
    // without re-running filter_sources (which could pick a different candidate).
    service.save_preference(
        &session
            .device
            .id,
    );

    // When no specific stream was requested (initial load, or media_source_id == item_id),
    // override source[0].Id to equal the item ID — clients expect this for auto-play.
    // Group and specific-stream requests keep their own UUIDs (specific_stream_requested = true).
    if !specific_stream_requested && !media_sources.is_empty() {
        media_sources[0].id = id;
        media_sources[0].e_tag = id;
    }

    for (source, (delivery_source_id, routes)) in media_sources
        .iter()
        .zip(sidecar_subtitle_routes)
    {
        // DeliveryUrl contains the source ID from apply_subtitle_delivery, while
        // some clients construct the route from the final MediaSourceInfo ID.
        // Cache both keys when auto-play rewrites the first source ID.
        if source.id != delivery_source_id {
            save_sidecar_subtitle_routes(
                &state.ctx,
                &session
                    .device
                    .id,
                id,
                source.id,
                routes.clone(),
            );
        }
        save_sidecar_subtitle_routes(
            &state.ctx,
            &session
                .device
                .id,
            id,
            delivery_source_id,
            routes,
        );
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
            // Route through our proxy so clients don't hit the raw IPTV URL directly
            // (which may redirect and confuse players that don't follow 302 on streams).
            source.is_remote = false;
            // Swiftfin skips the video-stream URL path for live items and falls back to
            // path, which is now a fake strm path. Always provide a real transcode_url
            // so Swiftfin takes that branch instead.
            if source
                .transcoding_url
                .is_none()
            {
                source.transcoding_url = Some(format!(
                    "/videos/{}/stream?Static=true&PlaySessionId={}&MediaSourceId={}&ApiKey={}",
                    id,
                    play_session_id,
                    source.id,
                    session
                        .device
                        .access_token
                        .expose(),
                ));
            }
        }
    }

    let error_code = if media_sources.is_empty() {
        media_sources.push(api::MediaSourceInfo {
            id,
            e_tag: id,
            name: Some("No streams found".to_string()),
            protocol: api::MediaProtocol::File,
            supports_direct_play: true,
            supports_direct_stream: true,
            supports_transcoding: true,
            path: Some("/videos/no-streams".to_string()),
            run_time_ticks: Some(10 * 3600 * 10_000_000),
            container: Some(VideoContainer::Mp4),
            bitrate: Some(100),
            size: Some(NO_STREAMS_VIDEO.len() as i64),
            formats: Some(vec![]),
            required_http_headers: Some(std::collections::HashMap::new()),
            media_streams: vec![api::MediaStream {
                type_: Some(api::MediaStreamType::Video),
                codec: Some("h264".to_string()),
                is_default: Some(true),
                index: 0,
                width: Some(640),
                height: Some(360),
                ..Default::default()
            }],
            ..Default::default()
        });
        Some(api::PlaybackErrorCode::NoCompatibleStream)
    } else {
        None
    };

    let info = api::PlaybackInfoResponse {
        media_sources,
        play_session_id: Some(play_session_id),
        // error_code,
        ..Default::default()
    };

    trace!(?info, "items_playbackinfo_result");
    Ok(Json(info))
}

static NO_STREAMS_VIDEO: &[u8] = include_bytes!("../../assets/no-streams.mp4");

fn no_streams_response() -> http::Response<Body> {
    http::Response::builder()
        .header(http::header::CONTENT_TYPE, "video/mp4")
        .header(http::header::CONTENT_LENGTH, NO_STREAMS_VIDEO.len())
        .body(Body::from(NO_STREAMS_VIDEO))
        .unwrap()
}

#[get("/videos/no-streams")]
pub async fn videos_no_streams() -> impl IntoResponse {
    no_streams_response()
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
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(mut q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    q.static_ = Some(true);
    let filename = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await
    .ok()
    .flatten()
    .map(|m| {
        m.stream_info
            .and_then(|si| si.filename)
            .unwrap_or_else(|| format!("{}.mkv", m.title))
    })
    .unwrap_or_else(|| "download.mkv".to_string());
    let safe = filename
        .replace('"', "")
        .replace('\\', "");
    let mut response = videos_stream_inner(
        headers,
        state,
        Some(
            session
                .user
                .id,
        ),
        id,
        q,
    )
    .await?
    .into_response();
    if let Ok(val) =
        http::HeaderValue::from_str(&format!("attachment; filename=\"{}\"", safe))
    {
        response
            .headers_mut()
            .insert(http::header::CONTENT_DISPOSITION, val);
    }
    Ok(response)
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
    videos_stream_inner(headers, state, None, id, q).await
}

#[get("/audio/{id}/stream.{container}")]
pub async fn audio_stream_by_container(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path((id, container)): Path<(Uuid, String)>,
    Query(mut q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    if q.container
        .is_none()
    {
        q.container = Some(container);
    }
    videos_stream_inner(headers, state, None, id, q).await
}

#[get("/videos/{id}/stream")]
pub async fn videos_stream(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path(id): Path<Uuid>,
    Query(q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    videos_stream_inner(headers, state, None, id, q).await
}

#[get("/videos/{id}/stream.{container}")]
pub async fn videos_stream_by_container(
    headers: headers::HeaderMap,
    State(state): State<AppState>,
    Path((id, container)): Path<(Uuid, String)>,
    Query(mut q): Query<api::VideoStreamQuery>,
) -> Result<impl IntoResponse> {
    if q.container
        .is_none()
    {
        q.container = Some(container);
    }
    videos_stream_inner(headers, state, None, id, q).await
}

fn ext_from_descriptor(descriptor: &crate::stream::StreamDescriptor) -> String {
    match descriptor {
        crate::stream::StreamDescriptor::Local(path) => path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("mkv")
            .to_string(),
        crate::stream::StreamDescriptor::Http { url, .. }
        | crate::stream::StreamDescriptor::Rtsp { url } => url
            .split('?')
            .next()
            .unwrap_or(url.as_str())
            .rsplit('.')
            .next()
            .filter(|e| !e.is_empty() && e.len() <= 5)
            .unwrap_or("mkv")
            .to_string(),
        crate::stream::StreamDescriptor::Torrent { file_hint, .. } => file_hint
            .as_deref()
            .and_then(|h| {
                std::path::Path::new(h)
                    .extension()
                    .and_then(|e| e.to_str())
            })
            .unwrap_or("mkv")
            .to_string(),
        _ => "mkv".to_string(),
    }
}

async fn videos_stream_inner(
    headers: headers::HeaderMap,
    state: AppState,
    user_id: Option<Uuid>,
    id: Uuid,
    q: api::VideoStreamQuery,
) -> Result<impl IntoResponse> {
    let media = StreamService::lookup(
        &state.ctx,
        id,
        q.media_source_id,
        q.device_id
            .as_deref(),
        user_id,
    )
    .await;

    let media = match media {
        Ok(m) => m,
        Err(_) => return Ok(no_streams_response().into_response()),
    };
    let Some(si) = media
        .stream_info
        .clone()
    else {
        return Ok(no_streams_response().into_response());
    };
    let descriptor = si.descriptor;

    // Direct play: serve bytes directly through the StreamSource trait.
    // This handles HTTP, local files, torrents, and opendal without going through
    // our own HTTP proxy — TorrentSource resolves and streams inline.
    if q.static_
        .unwrap_or(false)
    {
        // If the producing addon has http_redirect_stream enabled, issue a 302
        // directly to the stream URL instead of proxying bytes through remux.
        if let (Some(addon_id), crate::stream::StreamDescriptor::Http { url, .. }) =
            (si.addon_id, &descriptor)
        {
            if state
                .ctx
                .addons
                .get(addon_id)
                .map(|a| {
                    a.row
                        .http_redirect_stream
                })
                .unwrap_or(false)
            {
                return Ok(axum::response::Redirect::temporary(url).into_response());
            }
        }

        let resp = if let Some(addon_id) = descriptor.addon_id() {
            let addon = state
                .ctx
                .addons
                .get(addon_id)
                .context_not_found("addon not found")?;
            addon
                .stream
                .as_ref()
                .context_not_found("addon does not support streams")?
                .serve_stream(&descriptor, &headers)
                .await?
        } else {
            descriptor
                .clone()
                .into_source()
                .serve(&state, &headers)
                .await?
        };
        return Ok(resp.into_response());
    }

    let url = descriptor.server_input(
        media.id,
        state
            .ctx
            .config
            .port,
    );

    // Progressive transcode/remux: only reached when Static=false.
    let wants_stream_selection = q
        .audio_stream_index
        .is_some()
        || q.subtitle_stream_index
            .is_some();
    let container = q
        .container
        .as_deref()
        .unwrap_or("mp4")
        .to_string();
    let video_codec = q
        .video_codec
        .as_deref()
        .unwrap_or("copy");
    let encoding_opts = crate::db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    let video_transcode_enabled = encoding_opts
        .enable_video_transcoding
        .unwrap_or(true);
    let video_codec = if video_codec == "copy" || !video_transcode_enabled {
        "copy"
    } else {
        "h264"
    }
    .to_string();
    let audio_codec = q
        .audio_codec
        .unwrap_or_else(|| "aac".to_string());
    // Keep a copy before the video_codec is moved into params (needed for Content-Type logic)
    let is_copy_video = video_codec == "copy";

    info!(
        "starting progressive transcode for: {:?} (container={}, vcodec={}, acodec={}, start_ticks={:?}, bitrate={:?}, video_transcoding={})",
        &media.title,
        container,
        video_codec,
        audio_codec,
        q.start_time_ticks,
        q.video_bit_rate,
        encoding_opts
            .enable_video_transcoding
            .unwrap_or(true)
    );
    let source_video_stream = media
        .probe_data
        .as_ref()
        .and_then(|p| p.video_stream());
    let source_video_codec = source_video_stream
        .as_ref()
        .and_then(|s| {
            s.codec
                .clone()
        });
    let source_video_range_type = source_video_stream
        .as_ref()
        .and_then(|s| s.video_range_type);
    let source_hevc_params_out_of_band = source_video_stream
        .as_ref()
        .and_then(|s| {
            s.remux
                .as_ref()
        })
        .and_then(|r| r.hevc_params_out_of_band);
    let source_audio_codec = media
        .probe_data
        .as_ref()
        .and_then(|p| p.audio_stream())
        .and_then(|s| {
            s.codec
                .clone()
        });
    let burn_subtitle_prog = q
        .subtitle_method
        .as_deref()
        == Some("Encode");

    let params = crate::playback::engine::ProgressiveTranscodeParams {
        input_url: url,
        container: container.clone(),
        video_codec,
        audio_codec,
        start_time_ticks: q.start_time_ticks,
        max_width: q
            .max_width
            .map(|v| v as u32),
        max_height: q
            .max_height
            .map(|v| v as u32),
        video_bitrate: source_video_stream
            .and_then(|s| s.bit_rate)
            .map(|b| {
                let source = b as u32;
                q.video_bit_rate
                    .map_or(source, |v| source.min(v as u32))
            }),
        audio_bitrate: q
            .audio_bit_rate
            .map(|v| v as u32),
        audio_channels: q
            .audio_channels
            .map(|v| v as u32),
        audio_stream_index: q
            .audio_stream_index
            .map(|v| v as i32)
            .filter(|&v| v >= 0),
        subtitle_stream_index: q
            .subtitle_stream_index
            .map(|v| v as i32),
        burn_subtitle: burn_subtitle_prog,
        subtitle_width: None,
        subtitle_height: None,
        encoding_preset: encoding_opts.encoding_preset,
        source_video_codec,
        source_audio_codec,
        source_hevc_params_out_of_band,
        accelerator: hw_accel::from_encoding_opts(&encoding_opts),
        source_video_range_type,
        enable_tonemapping: encoding_opts
            .enable_tonemapping
            .unwrap_or(false),
        enable_vpp_tonemapping: encoding_opts
            .enable_vpp_tonemapping
            .unwrap_or(false),
        tonemapping_algorithm: encoding_opts
            .tonemapping_algorithm
            .unwrap_or_else(|| "hable".to_string()),
        tonemapping_desat: encoding_opts
            .tonemapping_desat
            .unwrap_or(0.0),
        tonemapping_peak: encoding_opts
            .tonemapping_peak
            .unwrap_or(0.0),
        allow_hevc_encoding: encoding_opts
            .allow_hevc_encoding
            .unwrap_or(false),
        allow_av1_encoding: encoding_opts
            .allow_av1_encoding
            .unwrap_or(false),
        h264_crf: encoding_opts
            .h264_crf
            .unwrap_or(23),
        h265_crf: encoding_opts
            .h265_crf
            .unwrap_or(28),
        normalize_audio_loudness: encoding_opts
            .normalize_audio_loudness
            .unwrap_or(false),
    };

    let stream = crate::playback::engine::start_progressive_transcode(params)?;
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

/// Returns additional parts for a multi-file video item.
#[get("/videos/{id}/additionalparts")]
pub async fn video_additional_parts(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    Ok(Json(api::BaseItemDtoQueryResult::default()))
}

#[get("/audio/{id}/universal")]
pub async fn audio_universal(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("track not found")?;

    state
        .ctx
        .addons
        .refresh_streams(
            &mut media,
            &state.ctx,
            Some(
                session
                    .user
                    .id,
            ),
        )
        .await
        .inspect_err(|e| error!("refresh_streams failed: {e:#}"));

    let play_session_id = q
        .play_session_id
        .unwrap_or_else(|| {
            common::get_uuid()
                .as_simple()
                .to_string()
        });

    let transcoding_url = format!(
        "/videos/{}/master.m3u8?PlaySessionId={}&MediaSourceId={}&VideoCodec=copy&AudioCodec=aac&ApiKey={}",
        id,
        play_session_id,
        id,
        session
            .device
            .access_token
            .expose()
    );

    Ok(axum::response::Redirect::temporary(&transcoding_url).into_response())
}

/// Bitrate test endpoint - returns a body of the requested size for bandwidth measurement.
#[get("/playback/bitratetest")]
pub async fn playback_bitratetest_sized(
    Query(q): Query<BitrateTestQuery>,
) -> Result<impl IntoResponse> {
    let size = q
        .size
        .unwrap_or(100_000)
        .min(10_000_000) as usize;
    let body = vec![0u8; size];
    Ok(Response::builder()
        .status(StatusCode::OK)
        .header("Content-Type", "application/octet-stream")
        .header("Content-Length", size.to_string())
        .body(Body::from(body))
        .unwrap())
}

#[query]
pub struct BitrateTestQuery {
    pub size: Option<u64>,
}

#[cfg(test)]
mod tests {
    use http::{StatusCode, header::HeaderValue};
    use remux_sdks::remux::VideoContainer;
    use serde_json::json;

    use crate::integration_test::{
        AUTH_HEADER, assert_api_keys_are_real, auth_header_with_token,
        authenticated_server, insert_test_source, insert_test_source_of_kind,
        insert_test_source_with_external_subtitle, new_test_server,
    };

    #[test]
    fn item_runtime_fills_missing_source_duration() {
        let mut source = crate::api::MediaSourceInfo::default();

        super::apply_item_runtime_fallback(&mut source, Some(142));

        assert_eq!(source.run_time_ticks, Some(1_420_000_000));
    }

    #[test]
    fn item_runtime_does_not_replace_probed_duration() {
        let mut source = crate::api::MediaSourceInfo {
            run_time_ticks: Some(900_000_000),
            ..Default::default()
        };

        super::apply_item_runtime_fallback(&mut source, Some(142));

        assert_eq!(source.run_time_ticks, Some(900_000_000));
    }

    #[test]
    fn parent_item_language_overrides_missing_or_stale_source_language() {
        let parent = crate::db::Media {
            original_language: Some("en".to_string()),
            ..Default::default()
        };

        assert_eq!(
            super::playback_original_language(Some(&parent), None),
            Some("en".to_string())
        );
        assert_eq!(
            super::playback_original_language(Some(&parent), Some("ru".to_string())),
            Some("en".to_string())
        );
        assert_eq!(
            super::playback_original_language(None, Some("fr".to_string())),
            Some("fr".to_string())
        );
    }

    #[tokio::test]
    async fn http_redirect_stream_issues_302_to_source_url() {
        use crate::{addons::addon::Addon, stream};
        use chrono::Utc;
        use remux_sdks::{remux::AddonPresetRef, stremio::ResourceType};
        use uuid::Uuid;

        let (server, guard, token) = authenticated_server().await;
        let ctx = &guard.0;
        let now = Utc::now().naive_utc();
        let addon_id = Uuid::new_v4();
        let stream_url = "https://cdn.example.com/movie.mp4";

        // Insert an addon with http_redirect_stream enabled.
        // The Stremio preset accepts any valid URL; the manifest fetch will fail but
        // the addon still lands in the runtime (failure is just warned about).
        let addon = Addon {
            id: addon_id,
            name: "redirect-test-addon".to_string(),
            preset: AddonPresetRef {
                kind: "stremio".to_string(),
                config: serde_json::json!({
                    "manifest_url": "https://example.com/addon/manifest.json"
                })
                .into(),
            },
            resources: vec![ResourceType::Stream],
            types: vec![],
            enabled: true,
            priority: 0,
            system: false,
            is_default: false,
            http_redirect_stream: true,
            service_filter: vec![],
            created_at: now,
            updated_at: now,
        };
        addon
            .insert(&ctx.db)
            .await
            .unwrap();

        // Reload the in-memory addon list so our new addon is visible to handlers.
        ctx.addons
            .reload(&ctx.db, &ctx.config)
            .await
            .unwrap();

        // Insert a media whose stream descriptor points at an HTTP URL and is
        // tagged with the addon that produced it.
        let mut media = crate::db::Media {
            title: "Redirect Test".to_string(),
            kind: crate::db::MediaKind::Stream,
            stream_info: Some(stream::StreamInfo {
                descriptor: stream::StreamDescriptor::http(stream_url),
                addon_id: Some(addon_id),
                ..Default::default()
            }),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(&ctx.db)
            .await
            .unwrap();

        // Expect a 3xx so axum-test doesn't reject it as non-2xx.
        let resp = server
            .get(&format!("/videos/{}/stream", media.id))
            .add_query_params([("Static", "true"), ("ApiKey", &token)])
            .expect_failure()
            .await;

        resp.assert_status(StatusCode::TEMPORARY_REDIRECT);
        assert_eq!(
            resp.header("location"),
            stream_url,
            "redirect Location must point directly to the source stream URL"
        );
    }

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
    async fn playback_stop_notifies_clients_that_user_data_changed() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;
        let play_session_id = "test-user-data-changed";
        let mut events = guard
            .0
            .ws_tx
            .subscribe();

        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id,
                "PlaySessionId": play_session_id,
                "PositionTicks": 0
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id,
                "PlaySessionId": play_session_id,
                "PositionTicks": 30_000_000i64
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        let changed_item_id =
            tokio::time::timeout(std::time::Duration::from_secs(1), async {
                loop {
                    if let crate::ws::WsEvent::UserDataChanged { item_id, .. } = events
                        .recv()
                        .await
                        .unwrap()
                    {
                        break item_id;
                    }
                }
            })
            .await
            .expect("playback stop should broadcast UserDataChanged");

        assert_eq!(changed_item_id, media.id);
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

    /// DirectPlay clients (e.g. Plezy) omit PlaySessionId from progress/stopped
    /// reports. The server must fall back to the active session for the device.
    #[tokio::test]
    async fn test_progress_and_stopped_without_play_session_id() {
        let (server, _ctx, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Start — no PlaySessionId (server generates one internally)
        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PositionTicks": 0,
                "CanSeek": true,
                "PlayMethod": "DirectPlay"
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // Progress — no PlaySessionId; device-based fallback must find the session
        server
            .post("/sessions/playing/progress")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PositionTicks": 300_000_000i64,
                "IsPaused": false,
                "IsMuted": false
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // Sessions endpoint must reflect the updated position
        let resp = server
            .get("/sessions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let sessions: Vec<crate::api::SessionInfoDto> = resp.json();
        let position = sessions[0]
            .play_state
            .as_ref()
            .and_then(|ps| ps.position_ticks);
        assert_eq!(
            position,
            Some(300_000_000),
            "position_ticks must be updated via device fallback"
        );

        // Stopped — also no PlaySessionId
        server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": "80ce1832bb797ffafaf65059b8b3dc9e",
                "PositionTicks": 600_000_000i64
            }))
            .await
            .assert_status(StatusCode::NO_CONTENT);

        // Session must be gone after stop
        let resp = server
            .get("/sessions")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        let sessions: Vec<crate::api::SessionInfoDto> = resp.json();
        assert!(
            sessions[0]
                .now_playing_item
                .is_none(),
            "session must have no now_playing_item after stop"
        );
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
        assert!(
            sessions[0]
                .now_playing_item
                .is_some()
        );
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
        assert_eq!(
            sessions[0]
                .device_name
                .as_deref(),
            Some("Chrome Laptop")
        );
        assert_eq!(
            sessions[0]
                .client
                .as_deref(),
            Some("Jellyfin Web")
        );
        assert_eq!(
            sessions[0]
                .application_version
                .as_deref(),
            Some("10.11.0")
        );
    }

    #[tokio::test]
    async fn test_playbackinfo_requires_auth() {
        let (server, _ctx) = new_test_server()
            .await
            .unwrap();
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
                "Id": media.id.simple().to_string(),
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
                "Id": media.id.simple().to_string(),
                "Container": "mp4",
                "RunTimeTicks": 100000000,
                "SupportsDirectPlay": true,
                "SupportsTranscoding": true
            }]
        }));
        // Sanity-check bitrate is probed and non-zero (exact value varies by probe).
        let body: serde_json::Value = resp.json();
        assert!(
            body["MediaSources"][0]["Bitrate"]
                .as_i64()
                .unwrap_or(0)
                > 0
        );
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
                "Id": media.id.simple().to_string(),
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

    /// The session token is carried in the URL the client is told to fetch, so
    /// it has to be the real one. It is wrapped in a `Secret`, which only ever
    /// prints as `<redacted>`.
    #[tokio::test]
    async fn test_playbackinfo_transcoding_url_carries_the_real_token() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source(&guard.0).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MaxStreamingBitrate": 1_000_000 }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let url = body["MediaSources"][0]["TranscodingUrl"]
            .as_str()
            .expect("TranscodingUrl should be present");
        assert!(
            url.contains(&format!("ApiKey={}", token)),
            "TranscodingUrl should carry the session token: {}",
            url
        );
        // Subtitle delivery URLs and any other URL in the body carry it too.
        assert_api_keys_are_real(&body, &token);
    }

    /// An external subtitle is delivered by URL, and that URL is built apart
    /// from the transcode one.
    #[tokio::test]
    async fn test_playbackinfo_subtitle_delivery_url_carries_the_real_token() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_test_source_with_external_subtitle(&guard.0).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MaxStreamingBitrate": 1_000_000 }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let delivery = body["MediaSources"][0]["MediaStreams"]
            .as_array()
            .expect("media streams")
            .iter()
            .find_map(|s| s["DeliveryUrl"].as_str())
            .expect("an external subtitle is delivered by URL");
        assert!(
            delivery.contains(&format!("ApiKey={}", token)),
            "subtitle delivery URL should carry the session token: {delivery}"
        );
        assert_api_keys_are_real(&body, &token);
    }

    /// Live TV takes its own branch and builds its own URL.
    #[tokio::test]
    async fn test_playbackinfo_live_url_carries_the_real_token() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media =
            insert_test_source_of_kind(&guard.0, crate::db::MediaKind::TvChannel).await;

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MaxStreamingBitrate": 1_000_000 }))
            .await;

        resp.assert_status_ok();
        assert_api_keys_are_real(&resp.json::<serde_json::Value>(), &token);
    }

    /// The audio redirect hands the client a URL it must fetch with the token
    /// already in it, so a redacted one 401s on the very next request.
    #[tokio::test]
    async fn test_audio_universal_redirect_carries_the_real_token() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media =
            insert_test_source_of_kind(&guard.0, crate::db::MediaKind::Track).await;

        let resp = server
            .get(&format!("/audio/{}/universal", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .expect_failure()
            .await;

        resp.assert_status(StatusCode::TEMPORARY_REDIRECT);
        let location = resp
            .header("location")
            .to_str()
            .unwrap()
            .to_string();
        assert!(
            location.contains(&format!("ApiKey={}", token)),
            "redirect should carry the session token: {location}"
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

    #[tokio::test]
    async fn test_playbackinfo_accepts_pgs_aliases_for_selected_subtitle() {
        use crate::api::{MediaSourceInfo, MediaStream, MediaStreamType};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let now = chrono::Utc::now().naive_utc();

        let mut media = crate::db::Media {
            title: "PGS Alias Test".to_string(),
            kind: crate::db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture.mkv".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(MediaSourceInfo {
                container: Some(VideoContainer::Mkv),
                default_subtitle_stream_index: Some(2),
                media_streams: vec![
                    MediaStream {
                        codec: Some("h264".to_string()),
                        type_: Some(MediaStreamType::Video),
                        index: 0,
                        width: Some(1920),
                        height: Some(1080),
                        ..Default::default()
                    },
                    MediaStream {
                        codec: Some("aac".to_string()),
                        type_: Some(MediaStreamType::Audio),
                        index: 1,
                        ..Default::default()
                    },
                    MediaStream {
                        codec: Some("hdmv_pgs_subtitle".to_string()),
                        type_: Some(MediaStreamType::Subtitle),
                        index: 2,
                        is_text_subtitle_stream: false,
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .expect("save media");

        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "SubtitleStreamIndex": 2,
                "DeviceProfile": {
                    "DirectPlayProfiles": [
                        { "Type": "Video", "Container": "*", "VideoCodec": "*", "AudioCodec": "*" }
                    ],
                    "SubtitleProfiles": [
                        { "Format": "pgs", "Method": "External" }
                    ],
                    "TranscodingProfiles": [],
                    "CodecProfiles": []
                }
            }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let source = &body["MediaSources"][0];
        let delivery_url = source["MediaStreams"][2]["DeliveryUrl"]
            .as_str()
            .expect("subtitle delivery url");
        let reasons = source["TranscodingReasons"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        assert!(
            delivery_url.contains("/Stream.sup?"),
            "expected PGS alias to map to SUP delivery, got {delivery_url}"
        );
        assert!(
            !reasons
                .iter()
                .any(|r| r.as_str() == Some("SubtitleCodecNotSupported")),
            "PGS alias should not force subtitle transcode: {reasons:?}"
        );
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
                "Id": media.id.simple().to_string(),
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
            body["MediaSources"][0]["Id"]
                .as_str()
                .unwrap(),
            source
                .id
                .simple()
                .to_string(),
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
            body2["MediaSources"][0]["Id"]
                .as_str()
                .unwrap(),
            source
                .id
                .simple()
                .to_string(),
            "source Id must equal item id when MediaSourceId == item id (Android TV)"
        );
    }

    /// A Movie with two Stream children and a specific MediaSourceId:
    /// - must return exactly one source
    /// - source Id must equal the requested MediaSourceId (never the other stream's id)
    /// - ETag must also match
    ///
    /// Without MediaSourceId, first source Id must equal the item (Movie) id.
    /// Both paths exercise the unconditional id-stamp that prevents probe fallback
    /// from leaking the fallback stream's UUID to the client.
    #[tokio::test]
    async fn test_playbackinfo_source_id_never_leaks_fallback_id() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let now = chrono::Utc::now().naive_utc();

        use crate::{
            api::{MediaSourceInfo, MediaStream, MediaStreamType},
            db,
        };

        let make_probe = || MediaSourceInfo {
            container: Some(VideoContainer::Mp4),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut movie = db::Media {
            title: "Test Track".to_string(),
            kind: db::MediaKind::Track,
            external_ids: db::ExternalIds {
                youtube_id: Some("test_track_id".to_string()),
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        movie
            .save(&ctx.db)
            .await
            .expect("save track");

        // Mark streams as already-refreshed so refresh_streams exits via the
        // TTL fast-path and never sets streams_refreshed_at to CURRENT_TIMESTAMP
        // (second-granularity). Without this, a second boundary crossed in slow
        // CI would make the staleness filter drop the test streams.
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(now)
            .bind(movie.id)
            .execute(&ctx.db)
            .await
            .expect("set streams_refreshed_at");

        let mut source_a = db::Media {
            title: "1080p".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(movie.id),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-1080p.mp4".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(make_probe()),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        source_a
            .save(&ctx.db)
            .await
            .expect("save source_a");

        let mut source_b = db::Media {
            title: "720p".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(movie.id),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-720p.mp4".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(make_probe()),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        source_b
            .save(&ctx.db)
            .await
            .expect("save source_b");

        // Specific source requested: must return exactly one source with that id.
        let resp = server
            .post(&format!("/items/{}/playbackinfo", movie.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MediaSourceId": source_a.id.to_string() }))
            .await;
        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert_eq!(
            body["MediaSources"]
                .as_array()
                .unwrap()
                .len(),
            1,
            "specific source requested: must return exactly one MediaSource"
        );
        assert_eq!(
            body["MediaSources"][0]["Id"]
                .as_str()
                .unwrap(),
            source_a
                .id
                .simple()
                .to_string(),
            "Id must equal the requested MediaSourceId, not source_b's id"
        );
        assert_eq!(
            body["MediaSources"][0]["ETag"]
                .as_str()
                .unwrap(),
            source_a
                .id
                .simple()
                .to_string(),
            "ETag must equal the requested MediaSourceId"
        );

        // No MediaSourceId: first source Id must equal the Movie id.
        let resp2 = server
            .post(&format!("/items/{}/playbackinfo", movie.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;
        resp2.assert_status_ok();
        let body2: serde_json::Value = resp2.json();
        assert_eq!(
            body2["MediaSources"][0]["Id"]
                .as_str()
                .unwrap(),
            movie
                .id
                .simple()
                .to_string(),
            "without MediaSourceId, first source Id must equal the item id, not a stream's id"
        );
        assert_eq!(
            body2["MediaSources"][0]["ETag"]
                .as_str()
                .unwrap(),
            movie
                .id
                .simple()
                .to_string(),
            "without MediaSourceId, ETag must equal the item id"
        );
    }

    #[tokio::test]
    async fn test_kill_active_encodings_no_session_is_noop() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .delete("/videos/activeencodings?DeviceId=test-device&PlaySessionId=nonexistent-session")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn test_kill_active_encodings_with_live_session_returns_204() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let psid = "kill-test-session";

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

        let resp = server
            .delete(&format!(
                "/videos/activeencodings?DeviceId=test-device&PlaySessionId={}",
                psid
            ))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;

        resp.assert_status(StatusCode::NO_CONTENT);
    }

    /// Full UserConfiguration JSON with sensible defaults. Merge in per-test
    /// overrides before posting to `/users/{id}/configuration`.
    fn default_user_config() -> serde_json::Value {
        json!({
            "PlayDefaultAudioTrack": true,
            "DisplayMissingEpisodes": false,
            "SubtitleMode": "Default",
            "EnableLocalPassword": false,
            "HidePlayedInLatest": true,
            "RememberAudioSelections": true,
            "RememberSubtitleSelections": true,
            "EnableNextEpisodeAutoPlay": true,
            "DisplayCollectionsView": false
        })
    }

    /// Merge `overrides` into `default_user_config()`.
    fn user_config_with(overrides: serde_json::Value) -> serde_json::Value {
        let mut base = default_user_config();
        if let (Some(base_obj), Some(ov_obj)) =
            (base.as_object_mut(), overrides.as_object())
        {
            for (k, v) in ov_obj {
                base_obj.insert(k.clone(), v.clone());
            }
        }
        base
    }

    /// Build a Stream source with Dutch (index 1) and English (index 2) audio tracks.
    async fn insert_multilang_source(ctx: &crate::AppContext) -> crate::db::Media {
        use crate::{
            api::{MediaSourceInfo, MediaStream, MediaStreamType},
            db,
        };
        let now = chrono::Utc::now().naive_utc();
        let probe = MediaSourceInfo {
            container: Some(VideoContainer::Mp4),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    language: Some("nl".to_string()),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 2,
                    language: Some("en".to_string()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut media = db::Media {
            title: "Multilang Test".to_string(),
            kind: db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-multilang.mp4".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(probe),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(&ctx.db)
            .await
            .expect("insert_multilang_source failed");
        media
    }

    /// `AudioLanguagePreference = "nl"` → Dutch track (index 1) selected as default.
    #[tokio::test]
    async fn test_audio_language_preference_selects_matching_track() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_multilang_source(&guard.0).await;

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();

        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "AudioLanguagePreference": "nl", "PlayDefaultAudioTrack": false }),
            ))
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultAudioStreamIndex"].as_i64(),
            Some(1),
            "Dutch track (index 1) should be selected when AudioLanguagePreference=nl"
        );
    }

    /// `AudioLanguagePreference` set to a language not present in the source
    /// → nothing matches and no stream is flagged default, so
    /// `DefaultAudioStreamIndex` is null.
    #[tokio::test]
    async fn test_audio_language_preference_no_match_leaves_unset() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_multilang_source(&guard.0).await;

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();

        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "AudioLanguagePreference": "de" }),
            ))
            .await;

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
            body["MediaSources"][0]["DefaultAudioStreamIndex"].is_null(),
            "With no German track present and no stream flagged default, DefaultAudioStreamIndex should be null"
        );
    }

    /// PlayDefaultAudioTrack=true selects the audio track whose language
    /// matches the item's `original_language` from DB metadata, even when it
    /// is not the container's first track (Dutch here, index 1).
    #[tokio::test]
    async fn test_play_default_audio_track_uses_original_language() {
        use crate::{
            api::{MediaSourceInfo, MediaStream, MediaStreamType},
            db,
        };
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        // Build a media item whose original_language is "en" but whose first
        // audio track is Dutch — the server must pick English (index 2).
        let now = chrono::Utc::now().naive_utc();
        let probe = MediaSourceInfo {
            container: Some(VideoContainer::Mp4),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    language: Some("nl".to_string()),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 2,
                    language: Some("en".to_string()),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut media = db::Media {
            title: "Original Lang Test".to_string(),
            kind: db::MediaKind::Stream,
            original_language: Some("en".to_string()),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-orig-lang.mp4".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(probe),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(
                &guard
                    .0
                    .db,
            )
            .await
            .expect("insert media failed");

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
        assert_eq!(
            body["MediaSources"][0]["DefaultAudioStreamIndex"].as_i64(),
            Some(2),
            "PlayDefaultAudioTrack=true should select the track matching original_language=en (index 2)"
        );
    }

    /// After reporting progress with `AudioStreamIndex=2`, the next PlaybackInfo
    /// request should recall that selection as `DefaultAudioStreamIndex`.
    #[tokio::test]
    async fn test_remember_audio_selections_recalls_saved_track() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_multilang_source(&guard.0).await;
        let psid = "recall-audio-test";

        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id.to_string(),
                "PlaySessionId": psid,
                "PositionTicks": 0
            }))
            .await;

        server
            .post("/sessions/playing/progress")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id.to_string(),
                "PlaySessionId": psid,
                "PositionTicks": 100_000_000i64,
                "AudioStreamIndex": 2
            }))
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultAudioStreamIndex"].as_i64(),
            Some(2),
            "Saved audio selection (index 2) should be recalled as DefaultAudioStreamIndex"
        );
    }

    /// With `RememberAudioSelections=false`, a track switch during playback must
    /// NOT be persisted and must NOT be recalled on the next PlaybackInfo request.
    #[tokio::test]
    async fn test_remember_audio_selections_false_does_not_persist() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_multilang_source(&guard.0).await;
        let psid = "no-recall-audio-test";

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();

        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "RememberAudioSelections": false }),
            ))
            .await;

        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id.to_string(),
                "PlaySessionId": psid,
                "PositionTicks": 0
            }))
            .await;

        server
            .post("/sessions/playing/progress")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({
                "ItemId": media.id.to_string(),
                "PlaySessionId": psid,
                "PositionTicks": 100_000_000i64,
                "AudioStreamIndex": 2
            }))
            .await;

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
            body["MediaSources"][0]["DefaultAudioStreamIndex"].is_null(),
            "With RememberAudioSelections=false, audio track switch must not be recalled and nothing is flagged default"
        );
    }

    /// When an item has multiple stream groups, each group's source ID in the
    /// initial PlaybackInfo response must be the StreamGroup UUID (not a stream UUID).
    /// Selecting a group by its UUID must return only streams from that group,
    /// not the first stream from the first group.
    #[tokio::test]
    async fn test_stream_group_selection_uses_group_uuid_and_plays_correct_stream() {
        use crate::{api, db};
        use remux_sdks::remux::{
            FilterMatchMode, SetOp, StreamFilter, StreamQuality, StreamResolution,
            StreamRule,
        };

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let now = chrono::Utc::now().naive_utc();

        // Groups: WEB (priority 0) and Blu-ray (priority 1)
        let web_group = crate::db::StreamGroup::create(
            &ctx.db,
            "1080p · WEB",
            StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::WebDl, StreamQuality::WebRip],
                    },
                ],
            },
            0,
        )
        .await
        .unwrap();

        let bluray_group = crate::db::StreamGroup::create(
            &ctx.db,
            "1080p · Blu-ray",
            StreamFilter {
                match_mode: FilterMatchMode::All,
                rules: vec![
                    StreamRule::Resolution {
                        op: SetOp::In,
                        values: vec![StreamResolution::R1080p],
                    },
                    StreamRule::Quality {
                        op: SetOp::In,
                        values: vec![StreamQuality::BluRay, StreamQuality::BluRayRemux],
                    },
                ],
            },
            1,
        )
        .await
        .unwrap();

        let make_probe = || api::MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                api::MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(api::MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                api::MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(api::MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };

        let mut movie = db::Media {
            title: "Test Movie".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt9999999").ok(),
                ..Default::default()
            },
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        movie.id = uuid::Uuid::from(&movie.media_id_raw());
        movie
            .save(&ctx.db)
            .await
            .unwrap();

        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(now)
            .bind(movie.id)
            .execute(&ctx.db)
            .await
            .unwrap();

        let mut web_stream = db::Media {
            title: "TestMovie.2026.1080p.WEB-DL.H264.mkv".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(movie.id),
            idx: Some(0),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "TestMovie.2026.1080p.WEB-DL.H264.mkv".into(),
                ),
                filename: Some("TestMovie.2026.1080p.WEB-DL.H264.mkv".to_string()),
                ..Default::default()
            }),
            probe_data: Some(make_probe()),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        web_stream
            .save(&ctx.db)
            .await
            .unwrap();

        let mut bluray_stream = db::Media {
            title: "TestMovie.2026.1080p.BluRay.x264.mkv".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(movie.id),
            idx: Some(1),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "TestMovie.2026.1080p.BluRay.x264.mkv".into(),
                ),
                filename: Some("TestMovie.2026.1080p.BluRay.x264.mkv".to_string()),
                ..Default::default()
            }),
            probe_data: Some(make_probe()),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        bluray_stream
            .save(&ctx.db)
            .await
            .unwrap();

        let resp = server
            .post(&format!("/items/{}/playbackinfo", movie.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({}))
            .await;
        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        let sources = body["MediaSources"]
            .as_array()
            .unwrap();

        assert_eq!(sources.len(), 2, "expected one source per group");
        // Source[0] (WEB group) gets its Id overridden to item_id
        assert_eq!(
            sources[0]["Id"]
                .as_str()
                .unwrap(),
            movie
                .id
                .simple()
                .to_string()
        );
        // Source[1] (Blu-ray group) must carry the StreamGroup UUID, not a stream UUID
        assert_eq!(
            sources[1]["Id"]
                .as_str()
                .unwrap(),
            bluray_group
                .id
                .simple()
                .to_string(),
            "blu-ray group source Id must be the StreamGroup UUID"
        );

        let resp2 = server
            .post(&format!("/items/{}/playbackinfo", movie.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "MediaSourceId": bluray_group.id.to_string() }))
            .await;
        resp2.assert_status_ok();
        let body2: serde_json::Value = resp2.json();
        let sources2 = body2["MediaSources"]
            .as_array()
            .unwrap();

        assert_eq!(
            sources2.len(),
            1,
            "specific group request must return one source"
        );
        // The source Id must remain the group UUID (not the item Id)
        assert_eq!(
            sources2[0]["Id"]
                .as_str()
                .unwrap(),
            bluray_group
                .id
                .simple()
                .to_string(),
            "source Id must be the Blu-ray group UUID"
        );
        // Path must reference the blu-ray stream file, not the WEB stream
        let path = sources2[0]["Path"]
            .as_str()
            .unwrap_or("");
        assert!(
            path.contains(
                &bluray_stream
                    .id
                    .to_string()
            ),
            "source Path must reference the Blu-ray stream ({}), got: {path}",
            bluray_stream.id
        );
    }

    /// When the client POSTs an explicit `AudioStreamIndex`, language preference
    /// must not override `DefaultAudioStreamIndex` in the response.
    #[tokio::test]
    async fn test_client_audio_index_skips_language_preference() {
        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let media = insert_multilang_source(&guard.0).await;

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();

        // Language preference would normally select Dutch (index 1)
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "AudioLanguagePreference": "nl" }),
            ))
            .await;

        // Client explicitly requests English (index 2)
        let resp = server
            .post(&format!("/items/{}/playbackinfo", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&json!({ "AudioStreamIndex": 2 }))
            .await;

        resp.assert_status_ok();
        let body: serde_json::Value = resp.json();
        assert_ne!(
            body["MediaSources"][0]["DefaultAudioStreamIndex"].as_i64(),
            Some(1),
            "Client's explicit AudioStreamIndex must prevent language preference from selecting Dutch (index 1)"
        );
    }

    /// Build a Stream with French (index 2) and English (index 3) subtitle tracks.
    async fn insert_subtitle_source(ctx: &crate::AppContext) -> crate::db::Media {
        use crate::{
            api::{MediaSourceInfo, MediaStream, MediaStreamType},
            db,
        };
        let now = chrono::Utc::now().naive_utc();
        let probe = MediaSourceInfo {
            container: Some(VideoContainer::Mkv),
            bitrate: Some(8_000_000),
            run_time_ticks: Some(100_000_000),
            media_streams: vec![
                MediaStream {
                    codec: Some("h264".to_string()),
                    type_: Some(MediaStreamType::Video),
                    index: 0,
                    width: Some(1920),
                    height: Some(1080),
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("aac".to_string()),
                    type_: Some(MediaStreamType::Audio),
                    index: 1,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("subrip".to_string()),
                    type_: Some(MediaStreamType::Subtitle),
                    index: 2,
                    language: Some("fra".to_string()),
                    is_text_subtitle_stream: true,
                    ..Default::default()
                },
                MediaStream {
                    codec: Some("subrip".to_string()),
                    type_: Some(MediaStreamType::Subtitle),
                    index: 3,
                    language: Some("eng".to_string()),
                    is_text_subtitle_stream: true,
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let mut media = db::Media {
            title: "Subtitle Fallback Test".to_string(),
            kind: db::MediaKind::Stream,
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::Local(
                    "test-fixture-subs.mkv".into(),
                ),
                ..Default::default()
            }),
            probe_data: Some(probe),
            created_at: now,
            updated_at: now,
            ..Default::default()
        };
        media
            .save(&ctx.db)
            .await
            .expect("insert_subtitle_source failed");
        media
    }

    /// When the user has no SubtitleLanguagePreference, the server's
    /// preferred_metadata_language fires as a fallback.
    #[tokio::test]
    async fn test_server_metadata_language_fallback_selects_subtitle() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let media = insert_subtitle_source(ctx).await;

        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: Some("fr".to_string()),
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&default_user_config())
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].as_i64(),
            Some(2),
            "server preferred_metadata_language 'fr' should select the French subtitle (index 2) when user has no preference"
        );
    }

    /// When the user has a SubtitleLanguagePreference that matches nothing, the server
    /// fallback must NOT fire — user preference wins even if it selects nothing.
    #[tokio::test]
    async fn test_server_fallback_does_not_fire_when_user_pref_set() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let media = insert_subtitle_source(ctx).await;

        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: Some("fr".to_string()),
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();
        // User prefers Japanese — not available in the fixture
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "SubtitleLanguagePreference": "jpn" }),
            ))
            .await;

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
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].is_null(),
            "server fallback must not fire when user has a preference set, even if it matches nothing; no stream is flagged default"
        );
    }

    /// Jellyfin web/SDK send `"SubtitleLanguagePreference": ""` (empty string, not
    /// null) when the user has not configured a subtitle language. The server
    /// metadata-language fallback must still fire in that case.
    #[tokio::test]
    async fn test_server_fallback_fires_when_user_pref_is_empty_string() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;
        let media = insert_subtitle_source(ctx).await;

        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: Some("fr".to_string()),
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();
        // Jellyfin sends an empty string when no subtitle language is configured
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&user_config_with(
                json!({ "SubtitleLanguagePreference": "" }),
            ))
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].as_i64(),
            Some(2),
            "empty-string SubtitleLanguagePreference should be treated as unset, so the server fallback (fr) selects French subtitle (index 2)"
        );
    }
    /// probe.rs sets `default_subtitle_stream_index` to the first subtitle stream
    /// unconditionally (ignoring the container's disposition flags). Because
    /// `apply_user_playback_prefs` only runs its preference/fallback blocks when
    /// `default_subtitle_stream_index.is_none()`, the server metadata-language
    /// fallback never fires for real probed media. This test reproduces that.
    #[tokio::test]
    async fn test_server_fallback_ignored_when_probe_pre_set_default() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;

        // Mimic probe_media(): default subtitle index pre-set to the first subtitle
        // (eng, index 3) regardless of what the server fallback would pick.
        let mut media = insert_subtitle_source(ctx).await;
        if let Some(pd) = media
            .probe_data
            .as_mut()
        {
            pd.default_subtitle_stream_index = Some(3);
        }
        media
            .save(&ctx.db)
            .await
            .expect("re-save media");

        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: Some("fr".to_string()),
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&default_user_config())
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].as_i64(),
            Some(2),
            "server metadata-language fallback (fr) should override the probe's container default (eng, index 3)"
        );
    }

    /// When the user has no subtitle preference AND the server has no metadata
    /// language, the container's default subtitle stream (the is_default flag
    /// recorded by the probe) is used as the last fallback — and the stream's
    /// is_default flag is left untouched.
    #[tokio::test]
    async fn test_container_default_subtitle_used_as_last_fallback() {
        use crate::{api::ServerConfiguration, db::Settings};

        let (server, guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);
        let ctx = &guard.0;

        // Probe-style: the container flags eng (index 3) as the default subtitle.
        let mut media = insert_subtitle_source(ctx).await;
        if let Some(pd) = media
            .probe_data
            .as_mut()
        {
            for s in &mut pd.media_streams {
                if matches!(s.type_, Some(crate::api::MediaStreamType::Subtitle)) {
                    s.is_default = Some(s.index == 3);
                }
            }
            pd.default_subtitle_stream_index = Some(3);
        }
        media
            .save(&ctx.db)
            .await
            .expect("re-save media");

        // No global metadata language configured at all.
        Settings::set_config(
            &ctx.db,
            &ServerConfiguration {
                preferred_metadata_language: None,
                ..ServerConfiguration::default()
            },
        )
        .await
        .expect("set server config");

        let me: serde_json::Value = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await
            .json();
        let user_id = me["Id"]
            .as_str()
            .unwrap();
        server
            .post(&format!("/users/{}/configuration", user_id))
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&default_user_config())
            .await;

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
        assert_eq!(
            body["MediaSources"][0]["DefaultSubtitleStreamIndex"].as_i64(),
            Some(3),
            "container default subtitle (eng, index 3) should be used when no user preference and no global metadata language exist"
        );
        // The stream's is_default flag must be left exactly as the container set it.
        let subs = &body["MediaSources"][0]["MediaStreams"];
        for s in subs
            .as_array()
            .unwrap()
        {
            if s["Type"] == "Subtitle" {
                assert_eq!(
                    s["IsDefault"]
                        .as_bool()
                        .unwrap(),
                    s["Index"]
                        .as_i64()
                        .unwrap()
                        == 3,
                    "subtitle is_default flags must be preserved untouched"
                );
            }
        }
    }
}
