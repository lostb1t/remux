use anyhow::anyhow;
use axum::Json;

fn ffmpeg_bin() -> String {
    std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into())
}
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
use remux_macros::{api_query, delete, get, post};
use serde::Deserialize;
use serde_json::json;
use serde_with::{DurationSeconds, serde_as};
use std::{io, time::Duration};
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, trace};
use url::Url;
use uuid::Uuid;

use crate::{
    AppState, api,
    api::MediaSourceInfoExt,
    common,
    common::{TickUnit, ToRunTimeTicks},
    db,
    db::auth,
};

use crate::{
    IntoApiError, OptionExt, ResultExt,
    device_profile::DeviceProfileExt,
    sdks,
    services::MediaResolveService,
    torrent,
    transcode::session::{TranscodeSession, TranscodeState},
};
use axum_anyhow::ApiResult as Result;

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
    items_playbackinfo_inner(state, session, id, payload).await
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

async fn items_playbackinfo_inner(
    state: AppState,
    session: auth::AuthSession,
    id: Uuid,
    q: api::PlaybackInfoQuery,
) -> Result<impl IntoResponse> {
    let media_source_id = q.media_source_id;

    trace!(?id, ?q, "items_playbackinfo");

    let device_profile = q.device_profile;

    let probe_cfg = db::Settings::get_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
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

    let mut media =
        MediaResolveService::resolve_item(media_source_id.unwrap_or(id), &state.ctx)
            .await?
            .context_not_found("not found")?;

    // When a StreamGroup UUID is requested, resolve it to the group's best candidate
    // and keep all candidates (including subsequent groups) for probe fallback scope.
    let group_source_override: Option<(Uuid, String, Vec<db::Media>)> = if media.kind
        == db::MediaKind::StreamGroup
    {
        let gid = media.id;
        let gtitle = media
            .title
            .clone();
        let mut candidates = db::StreamGroup::streams_for(
            &state
                .ctx
                .db,
            &gid,
            &id,
        )
        .await?;
        if candidates.is_empty() {
            return Err(anyhow::anyhow!("no streams available for this group").into());
        }
        // Append streams from lower-priority groups so probe can cascade across groups.
        let cascade = db::StreamGroup::streams_for_groups_after(
            &state
                .ctx
                .db,
            &gid,
            &id,
        )
        .await
        .unwrap_or_default();
        candidates.extend(cascade);
        media = candidates[0].clone();
        Some((gid, gtitle, candidates))
    } else {
        None
    };

    // Load the top-level Movie/Episode for subtitle lookup.
    // `id` is always the movie/episode UUID; `media_source_id` may point to a
    // child Source, so we always resolve via `id` to get the IMDB fields.
    let subtitle_media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
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
        state
            .ctx
            .addons
            .refresh_streams(&mut media, &state.ctx)
            .await
            .inspect_err(|e| error!("refresh_streams failed: {e:#}"));
        let sources = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?;
        let raw = if sources.is_empty() {
            vec![media]
        } else {
            sources
        };
        {
            let sources = db::StreamGroup::filter_sources(
                &state
                    .ctx
                    .db,
                raw,
                show_ungrouped,
            )
            .await;
            if let Some(sf) = session
                .user
                .policy
                .as_ref()
                .and_then(|p| {
                    p.stream_filter
                        .as_ref()
                })
                .filter(|sf| {
                    !sf.rules
                        .is_empty()
                })
            {
                let before = sources.len();
                let filtered = db::apply_stream_filter(sf, sources);
                tracing::debug!(
                    user = %session.user.username,
                    sources_before = before,
                    sources_after = filtered.len(),
                    rules = sf.rules.len(),
                    "stream filter applied"
                );
                filtered
            } else {
                tracing::debug!(
                    user = %session.user.username,
                    has_policy = session.user.policy.is_some(),
                    has_stream_filter = session.user.policy.as_ref().and_then(|p| p.stream_filter.as_ref()).is_some(),
                    "stream filter skipped"
                );
                sources
            }
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
        .map(|sid| {
            sid != id
                && all_source_medias
                    .iter()
                    .any(|s| s.id == sid)
        })
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

    let probe_timeout_secs = probe_cfg
        .probe_timeout_secs
        .unwrap_or(20) as u64;
    let probe_timeout_p2p_secs = probe_cfg
        .probe_timeout_p2p_secs
        .unwrap_or(60) as u64;
    let auto_next_stream = probe_cfg
        .auto_next_stream_on_probe_fail
        .unwrap_or(true);
    let max_stream_retries = probe_cfg
        .max_probe_fallback_streams
        .unwrap_or(3) as usize;

    let port = state
        .ctx
        .config
        .port;
    // For group selections, use all group candidates (including cascade) as the probe fallback pool.
    // Resolution filtering is disabled for group requests: the group priority order already
    // encodes the user's quality preference, so cross-resolution fallback is intentional.
    let (all_sources, restrict_resolution) =
        if let Some((_, _, ref candidates)) = group_source_override {
            (candidates.clone(), false)
        } else {
            (source_medias.clone(), true)
        };
    let mut media_sources = Vec::with_capacity(source_medias.len());
    for (idx, sm) in source_medias
        .into_iter()
        .enumerate()
    {
        let url_opt = sm
            .stream_info
            .as_ref()
            .map(|si| {
                si.descriptor
                    .server_input(sm.id, port)
            });
        let skip_probe = probe_only_first && idx > 0;
        let timeout_secs = if sm
            .stream_info
            .as_ref()
            .map_or(false, |si| si.is_p2p())
        {
            probe_timeout_p2p_secs
        } else {
            probe_timeout_secs
        };
        let mut source = probe_source(
            &sm,
            url_opt.clone(),
            skip_probe,
            timeout_secs,
            auto_next_stream,
            max_stream_retries,
            &all_sources,
            restrict_resolution,
            port,
            &state
                .ctx
                .db,
        )
        .await?;
        source.id = sm.id;
        source.e_tag = sm.id;
        source.name = Some(
            sm.title
                .clone(),
        );
        source.has_segments = true;
        source.path = Some(format!("/remux/{}", sm.id));
        source.is_remote = false;

        // Re-apply binge-group headers on top of the probed result —
        // ffmpeg probing produces a fresh `MediaSourceInfo` and would
        // otherwise drop the `X-Remux-BingeGroup` / `X-Gelato-BingeGroup`
        // hints we stashed alongside the source.
        source.remux = Some(api::MediaSourceRemuxInfo {
            provider_info: sm
                .stream_info
                .as_ref()
                .and_then(|si| serde_json::to_value(si).ok()),
        });

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
                sm.stream_info
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

        // Image-based subtitles (PGS/DVD) can't be rendered by web clients — detect
        // from the explicitly-selected or default subtitle stream and add a transcode reason.
        let effective_sub_idx = q
            .subtitle_stream_index
            .or(source.default_subtitle_stream_index);
        let needs_pgs_burn = effective_sub_idx.map_or(false, |idx| {
            source
                .media_streams
                .iter()
                .any(|s| {
                    s.index == idx
                        && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                        && matches!(
                            s.codec
                                .as_deref()
                                .unwrap_or(""),
                            "pgssub" | "hdmv_pgs_subtitle" | "dvd_subtitle" | "dvdsub"
                        )
                })
        });
        if needs_pgs_burn {
            let codec = effective_sub_idx
                .and_then(|idx| {
                    source
                        .media_streams
                        .iter()
                        .find(|s| s.index == idx)
                })
                .and_then(|s| {
                    s.codec
                        .clone()
                })
                .unwrap_or_default();
            transcode_reasons
                .insert(api::TranscodeReason::SubtitleCodecNotSupported(codec));
        }

        // `EnableTranscoding=true` means "allowed", not "forced".
        let transcode_required = !transcode_reasons.is_empty()
            || !q
                .enable_direct_play
                .unwrap_or(true)
            || !q
                .enable_direct_stream
                .unwrap_or(true);
        let needs_transcoding = transcode_required
            && q.enable_transcoding
                .unwrap_or(true);

        tracing::debug!(
            source_id = %sm.id,
            transcode_reasons = ?transcode_reasons,
            "playback decision"
        );

        if needs_transcoding {
            let is_audio_only = source
                .video_stream()
                .is_none();

            if is_audio_only {
                let trans_profile = device_profile
                    .as_ref()
                    .and_then(|p| p.audio_transcoding_profile());
                let trans_container = trans_profile
                    .and_then(|p| {
                        p.container
                            .clone()
                    })
                    .unwrap_or_else(|| "mp3".to_string());
                let audio_codec = trans_profile
                    .and_then(|p| {
                        p.audio_codec
                            .as_deref()
                    })
                    .and_then(|c| {
                        c.split(',')
                            .next()
                    })
                    .map(|c| {
                        c.trim()
                            .to_string()
                    })
                    .unwrap_or_else(|| "aac".to_string());

                let start_time_param = q
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
                    session
                        .device
                        .access_token,
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
                            p.container
                                .clone()
                                .unwrap_or_else(|| "ts".to_string()),
                            p.protocol
                                .clone()
                                .unwrap_or_else(|| "hls".to_string()),
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

                let video_transcode_allowed = encoding_cfg
                    .enable_video_transcoding
                    .unwrap_or(true)
                    && session
                        .user
                        .policy
                        .as_ref()
                        .map(|p| p.enable_video_playback_transcoding)
                        .unwrap_or(true);

                if needs_video_transcode && !video_transcode_allowed {
                    info!(
                        user = %session.user.username,
                        source_id = %sm.id,
                        "video transcoding required but not allowed — marking source as not transcodable"
                    );
                    source.supports_transcoding = false;
                    source.supports_direct_play = false;
                    source.supports_direct_stream = false;
                    continue;
                }

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
                        source
                            .media_streams
                            .iter()
                            .find(|s| {
                                s.index == idx
                                    && matches!(
                                        s.type_,
                                        Some(api::MediaStreamType::Subtitle)
                                    )
                            })
                    })
                    .and_then(|stream| {
                        let codec = stream
                            .codec
                            .as_deref()
                            .unwrap_or("");
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
                let audio_stream_param = q
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
                let start_time_param = q
                    .start_time_ticks
                    .map(|t| format!("&StartTimeTicks={}", t))
                    .unwrap_or_default();

                source.supports_transcoding = true;
                source.transcoding_url = Some(
                    if trans_protocol.eq_ignore_ascii_case("hls") {
                        format!(
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
                            session
                                .device
                                .access_token,
                        )
                    } else {
                        format!(
                            "/videos/{}/stream.{}?PlaySessionId={}&MediaSourceId={}&VideoCodec={}&AudioCodec={}{}{}{}{}{}{}&ApiKey={}",
                            id,
                            trans_container,
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
                            session
                                .device
                                .access_token,
                        )
                    },
                );
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
        }

        // Set delivery URL on text subtitle streams so clients can download them.
        let source_id = source.id;
        let api_key = &session
            .device
            .access_token;
        for stream in source
            .media_streams
            .iter_mut()
        {
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

        // For group selections, expose the stable group UUID to the client.
        // TranscodingUrl already embeds the real source UUID (set before this point).
        if let Some((gid, ref gtitle, _)) = group_source_override {
            source.id = gid;
            source.e_tag = gid;
            source.name = Some(gtitle.clone());
        }

        media_sources.push(source);
    }

    // Inject external subtitles from AIO (cache-backed)
    if let Some(ref sm) = subtitle_media {
        let sub_langs = probe_cfg
            .subtitle_languages
            .clone()
            .unwrap_or_default();
        inject_external_subtitles(
            &state.ctx,
            sm,
            &mut media_sources,
            id,
            &session
                .device
                .access_token,
            sub_langs,
        )
        .await;
    }

    // Apply per-user playback preferences
    apply_user_playback_prefs(
        &state
            .ctx
            .db,
        &session.user,
        &id,
        &mut media_sources,
    )
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
                        .access_token,
                ));
            }
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

/// Resolve probe data for a single source: cache hit → skip → live probe with fallback.
async fn probe_source(
    sm: &db::Media,
    url_opt: Option<String>,
    skip_probe: bool,
    timeout_secs: u64,
    auto_next_stream: bool,
    max_retries: usize,
    all_sources: &[db::Media],
    restrict_resolution: bool,
    port: u16,
    db: &sqlx::SqlitePool,
) -> axum_anyhow::ApiResult<api::MediaSourceInfo> {
    if skip_probe {
        return Ok(api::MediaSourceInfo::from(sm.clone()));
    }
    if let Some(cached) = &sm.probe_data {
        if cached
            .video_stream()
            .is_some()
        {
            tracing::debug!(id = %sm.id, "probe cache hit");
            return Ok(cached.clone());
        }
        tracing::debug!(id = %sm.id, "probe cache stale (no video stream), re-probing");
    }
    probe_with_fallback(
        sm.clone(),
        url_opt,
        timeout_secs,
        auto_next_stream,
        max_retries,
        all_sources,
        restrict_resolution,
        port,
        db,
    )
    .await
}

/// Probe a stream URL, retrying with the next matching candidate on failure.
///
/// Returns a 500 error if all candidates fail to probe.
async fn probe_with_fallback(
    primary: db::Media,
    primary_url: Option<String>,
    timeout_secs: u64,
    auto_next_stream: bool,
    max_retries: usize,
    all_sources: &[db::Media],
    restrict_resolution: bool,
    port: u16,
    db: &sqlx::SqlitePool,
) -> axum_anyhow::ApiResult<api::MediaSourceInfo> {
    use crate::{IntoApiError, ResultExt};

    let candidates: Vec<(db::Media, String)> = if auto_next_stream {
        let pri_p2p = primary
            .stream_info
            .as_ref()
            .map_or(false, |si| si.is_p2p());
        let pri_res = primary
            .stream_info
            .as_ref()
            .and_then(|si| si.resolution_tag());
        all_sources
            .iter()
            .filter(|c| {
                if c.id == primary.id {
                    return false;
                }
                let c_p2p = c
                    .stream_info
                    .as_ref()
                    .map_or(false, |si| si.is_p2p());
                if c_p2p != pri_p2p {
                    return false;
                }
                if restrict_resolution {
                    let c_res = c
                        .stream_info
                        .as_ref()
                        .and_then(|si| si.resolution_tag());
                    if c_res != pri_res {
                        return false;
                    }
                }
                true
            })
            // In group-cascade mode (restrict_resolution=false) try all candidates;
            // otherwise honour the configured retry cap.
            .take(if restrict_resolution {
                max_retries
            } else {
                usize::MAX
            })
            .filter_map(|c| {
                let url = c
                    .stream_info
                    .as_ref()?
                    .descriptor
                    .server_input(c.id, port);
                Some((c.clone(), url))
            })
            .collect()
    } else {
        vec![]
    };

    let all_to_try = std::iter::once((primary.clone(), primary_url)).chain(
        candidates
            .into_iter()
            .map(|(m, u)| (m, Some(u))),
    );

    for (sm, url_opt) in all_to_try {
        let is_retry = sm.id != primary.id;
        let url = match url_opt {
            Some(u) => u,
            None => {
                return Err(anyhow!("stream has no URL"))
                    .context_internal("stream has no URL");
            }
        };
        if is_retry {
            tracing::info!(
                failed_id = %primary.id,
                next_id = %sm.id,
                next_url = %url,
                "probe failed, trying next matching stream"
            );
        }
        let url2 = url.clone();
        let sm2 = sm.clone();
        let db2 = db.clone();
        let probe_result = tokio::time::timeout(
            std::time::Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(move || {
                crate::transcode::probing::probe_media(&url2)
            }),
        )
        .await;

        match probe_result {
            Ok(Ok(Ok((mut probed, segments)))) => {
                if probed
                    .video_stream()
                    .is_some()
                    || probed
                        .audio_stream()
                        .is_some()
                {
                    if !segments.is_empty() {
                        probed.segments = Some(segments);
                    }
                    if let Err(e) =
                        db::Media::save_probe_data(&db2, &sm2.id, &probed).await
                    {
                        tracing::warn!(id = %sm2.id, error = %e, "failed to save probe data");
                    }
                } else {
                    tracing::warn!(id = %sm2.id, "probe returned no audio or video stream, not caching");
                }
                return Ok(probed);
            }
            Ok(Ok(Err(e))) => {
                tracing::warn!(url = %url, error = %e, "probe failed");
            }
            Ok(Err(e)) => {
                tracing::warn!(url = %url, error = %e, "probe task panicked");
            }
            Err(_) => {
                tracing::warn!(url = %url, timeout = timeout_secs, "probe timed out");
            }
        }
    }

    Err(anyhow!(
        "all probe attempts failed for stream {}",
        primary.id
    ))
    .context_internal("stream probe failed — no usable streams found")
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
    if q.container
        .is_none()
    {
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
    if q.container
        .is_none()
    {
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
    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &q.media_source_id
            .unwrap_or(id),
    )
    .await?
    .context_not_found("not found")?;

    if media.kind == db::MediaKind::Movie
        || media.kind == db::MediaKind::Episode
        || media.kind == db::MediaKind::Track
    {
        let sources = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?;
        media = if let Some(wanted) = q.media_source_id {
            sources
                .iter()
                .find(|s| s.id == wanted)
                .cloned()
        } else {
            None
        }
        .or_else(|| {
            sources
                .into_iter()
                .next()
        })
        .context_not_found("no playable source found")?;
    }

    let descriptor = media
        .stream_info
        .map(|si| si.descriptor)
        .context_not_found("media source has no URL")?;

    // Direct play: serve bytes directly through the StreamSource trait.
    // This handles HTTP, local files, torrents, and opendal without going through
    // our own HTTP proxy — TorrentSource resolves and streams inline.
    if q.static_
        .unwrap_or(false)
    {
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
        return Ok(resp);
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
        "starting progressive transcode for: {:?} (container={}, vcodec={}, acodec={}, start_ticks={:?}, bitrate={:?})",
        &media.title,
        container,
        video_codec,
        audio_codec,
        q.start_time_ticks,
        q.video_bit_rate
    );

    let encoding_opts = crate::db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
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
    let params = crate::transcode::engine::ProgressiveTranscodeParams {
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
        video_bitrate: q
            .video_bit_rate
            .map(|v| v as u32),
        audio_bitrate: q
            .audio_bit_rate
            .map(|v| v as u32),
        audio_channels: q
            .audio_channels
            .map(|v| v as u32),
        audio_stream_index: q
            .audio_stream_index
            .map(|v| v as i32),
        subtitle_stream_index: q
            .subtitle_stream_index
            .map(|v| v as i32),
        burn_subtitle: q
            .subtitle_method
            .as_deref()
            == Some("Encode"),
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
        vaapi_driver: encoding_opts
            .vaapi_driver
            .unwrap_or_default(),
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
    auth::Device::delete_by_access_token(
        &state
            .ctx
            .db,
        &session
            .device
            .access_token,
    )
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
        let _ = auth::Device::save_capabilities(
            &state
                .ctx
                .db,
            &session
                .device
                .id,
            &caps,
        )
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
        let _ = auth::Device::save_capabilities(
            &state
                .ctx
                .db,
            &id,
            &caps,
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[post("/sessions/playing")]
pub async fn report_playback_start(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackInfo>,
) -> Result<impl IntoResponse> {
    state
        .ctx
        .sessions
        .start(
            &state
                .ctx
                .db,
            &session,
            &data,
        )
        .await
        .map_err(|e| {
            // Re-raise stream-limit errors as 403 Forbidden; everything else as 500.
            if e.to_string()
                .contains("Stream limit reached")
            {
                e.context_forbidden("Maximum concurrent streams reached")
            } else {
                e.context_internal("failed to start session")
            }
        })?;
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::SessionsChanged);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[cfg(test)]
mod tests {
    use http::{StatusCode, header::HeaderValue};
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
            body["MediaSources"][0]["Id"]
                .as_str()
                .unwrap(),
            source
                .id
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
            container: Some("mp4".to_string()),
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

        let mut source_a = db::Media {
            title: "1080p".to_string(),
            kind: db::MediaKind::Stream,
            parent_id: Some(movie.id),
            stream_info: Some(crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::http(
                    "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/1080/Big_Buck_Bunny_1080_10s_5MB.mp4"
                        .to_string(),
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
                descriptor: crate::stream::StreamDescriptor::http(
                    "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/720/Big_Buck_Bunny_720_10s_5MB.mp4"
                        .to_string(),
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
                .to_string(),
            "Id must equal the requested MediaSourceId, not source_b's id"
        );
        assert_eq!(
            body["MediaSources"][0]["ETag"]
                .as_str()
                .unwrap(),
            source_a
                .id
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
                .to_string(),
            "without MediaSourceId, first source Id must equal the item id, not a stream's id"
        );
        assert_eq!(
            body2["MediaSources"][0]["ETag"]
                .as_str()
                .unwrap(),
            movie
                .id
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
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        state
            .ctx
            .sessions
            .progress(
                &state
                    .ctx
                    .db,
                &session.user,
                psid,
                &data,
            )
            .await
            .context_internal("failed to update progress")?;
        let _ = state
            .ctx
            .ws_tx
            .send(crate::ws::WsEvent::SessionsChanged);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/playing/stopped")]
pub async fn report_playback_stopped(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackInfo>,
) -> Result<impl IntoResponse> {
    if let Some(ref psid) = data.play_session_id {
        state
            .ctx
            .sessions
            .stopped(
                &state
                    .ctx
                    .db,
                &session.user,
                psid,
                &data,
            )
            .await
            .context_internal("failed to record stop")?;
        let _ = state
            .ctx
            .ws_tx
            .send(crate::ws::WsEvent::SessionsChanged);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[api_query]
pub struct PingQuery {
    pub play_session_id: String,
}

#[post("/sessions/playing/ping")]
pub async fn ping_playback_session(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<PingQuery>,
) -> Result<impl IntoResponse> {
    state
        .ctx
        .sessions
        .ping(&q.play_session_id);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/capabilities/full")]
pub async fn sessions_capabilities_full(
    State(state): State<AppState>,
    session: auth::AuthSession,
    body: Option<Json<api::ClientCapabilitiesDto>>,
) -> Result<StatusCode> {
    if let Some(Json(caps)) = body {
        let _ = auth::Device::save_capabilities(
            &state
                .ctx
                .db,
            &session
                .device
                .id,
            &caps,
        )
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
        let _ = auth::Device::save_capabilities(
            &state
                .ctx
                .db,
            &id,
            &caps,
        )
        .await;
    }
    Ok(StatusCode::NO_CONTENT)
}

#[serde_as]
#[api_query]
#[derive(Default)]
struct SessionsQuery {
    #[serde(rename = "activeWithinSeconds", alias = "ActiveWithinSeconds")]
    #[serde_as(as = "Option<DurationSeconds<u64>>")]
    active_within: Option<Duration>,
    device_id: Option<String>,
    controllable_by_user_id: Option<Uuid>,
}

/// Get all active sessions
#[get("/sessions")]
pub async fn get_sessions(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Query(q): Query<SessionsQuery>,
) -> Result<impl IntoResponse> {
    let mut devices = auth::Device::get_all(
        &state
            .ctx
            .db,
        q.active_within,
    )
    .await?;
    if let Some(ref did) = q.device_id {
        devices.retain(|d| &d.id == did);
    }
    let playback_sessions = state
        .ctx
        .sessions
        .get_all();

    let mut sessions = Vec::with_capacity(devices.len());
    for device in devices {
        // Prefer a session that has an active transcode attached; fall back to
        // any session for this device. This handles quality-switch windows where
        // a new stub (with inherited device_id) coexists with the old session
        // that had its transcode cleared.
        let ps = playback_sessions
            .iter()
            .filter(|s| s.device_id == device.id)
            .max_by_key(|s| {
                (
                    s.transcode
                        .is_some(),
                    s.last_activity,
                )
            });

        // Load full media from DB if there's an active playback session.
        let mut media = if let Some(ps) = ps {
            db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &ps.item_id,
            )
            .await
            .ok()
            .flatten()
        } else {
            None
        };

        // Load the source being played directly by media_source_id.
        // The source has probe_data with MediaStreams from ffprobe.
        let mut source_media = if let Some(msid) = ps.and_then(|p| {
            p.media_source_id
                .as_ref()
        }) {
            if let Ok(source_id) = msid.parse::<Uuid>() {
                db::Media::get_by_id(
                    &state
                        .ctx
                        .db,
                    &source_id,
                )
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
            .and_then(|m| {
                m.probe_data
                    .as_ref()
            })
            .is_none()
        {
            if let Some(m) = media.as_mut() {
                if let Ok(sources) = m
                    .streams(
                        &state
                            .ctx
                            .db,
                    )
                    .await
                {
                    if let Some(s) = sources
                        .into_iter()
                        .find(|s| {
                            s.probe_data
                                .is_some()
                        })
                    {
                        source_media = Some(s);
                    }
                }
            }
        }

        let probe_data = source_media
            .as_ref()
            .and_then(|m| {
                m.probe_data
                    .as_ref()
            })
            .or_else(|| {
                media
                    .as_ref()
                    .and_then(|m| {
                        m.probe_data
                            .as_ref()
                    })
            });

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
            if let Some(probe) = probe_data {
                if !probe
                    .media_streams
                    .is_empty()
                {
                    if item
                        .media_streams
                        .is_none()
                    {
                        item.media_streams = Some(
                            probe
                                .media_streams
                                .clone(),
                        );
                    }
                    // Populate MediaStreams inside each MediaSource so clients
                    // that read streams from the source (e.g. Streamyfin) get
                    // the full track list even in the Sessions response.
                    if let Some(ref mut sources) = item.media_sources {
                        for source in sources.iter_mut() {
                            if source
                                .media_streams
                                .is_empty()
                            {
                                source.media_streams = probe
                                    .media_streams
                                    .clone();
                            }
                        }
                    }
                }
            }
            Some(item)
        } else {
            None
        };

        // Attach TranscodingInfo with enriched metadata from probe data.
        let transcode_guard = if let Some(ts) = ps.and_then(|ps| {
            ps.transcode
                .clone()
        }) {
            Some(
                ts.read_owned()
                    .await,
            )
        } else {
            None
        };
        let transcoding_info = transcode_guard
            .as_deref()
            .map(|ts| {
                // Pull width/height/bitrate/channels from source media probe data.
                let video_stream = probe_data.and_then(|p| {
                    p.media_streams
                        .iter()
                        .find(|s| s.type_ == Some(api::MediaStreamType::Video))
                });
                let width = video_stream.and_then(|v| {
                    v.width
                        .map(|x| x as i32)
                });
                let height = video_stream.and_then(|v| {
                    v.height
                        .map(|x| x as i32)
                });
                let audio_channels = probe_data.and_then(|p| {
                    p.media_streams
                        .iter()
                        .find(|s| s.type_ == Some(api::MediaStreamType::Audio))
                        .and_then(|a| {
                            a.channels
                                .map(|x| x as i32)
                        })
                });

                // Compute completion percentage from transcode progress.
                let completion_percentage = if ts.runtime_ticks > 0 {
                    let start_ticks = (ts.start_time_secs as i64)
                        .to_ticks(TickUnit::Seconds)
                        .unwrap_or(0);
                    let last_seg = ts
                        .last_segment_index
                        .load(std::sync::atomic::Ordering::Relaxed)
                        as i64;
                    let transcoded_ticks = start_ticks
                        + ((last_seg + 1) * ts.segment_length as i64)
                            .to_ticks(TickUnit::Seconds)
                            .unwrap_or(0);
                    Some(
                        (transcoded_ticks as f64 / ts.runtime_ticks as f64 * 100.0)
                            .min(100.0),
                    )
                } else {
                    None
                };

                // Use the actual source codec name when ffmpeg is copying the
                // stream (remux). Clients use VideoCodec/AudioCodec to display
                // the stream format, so "copy" is meaningless to them.
                let video_codec_name = if ts.video_codec == "copy" {
                    ts.source_video_codec
                        .clone()
                        .unwrap_or_else(|| {
                            ts.video_codec
                                .clone()
                        })
                } else {
                    ts.video_codec
                        .clone()
                };
                let audio_codec_name = if ts.audio_codec == "copy" {
                    ts.source_audio_codec
                        .clone()
                        .unwrap_or_else(|| {
                            ts.audio_codec
                                .clone()
                        })
                } else {
                    ts.audio_codec
                        .clone()
                };

                api::TranscodingInfo {
                    audio_codec: Some(audio_codec_name),
                    video_codec: Some(video_codec_name),
                    container: Some("ts".to_string()),
                    is_video_direct: ts.video_codec == "copy",
                    is_audio_direct: ts.audio_codec == "copy",
                    bitrate: probe_data.and_then(|p| p.bitrate),
                    width,
                    height,
                    audio_channels,
                    completion_percentage,
                    transcode_reasons: ts
                        .transcode_reasons
                        .clone(),
                    ..Default::default()
                }
            });

        // Build PlayState from active playback session, always non-null.
        let play_state = Some(
            ps.map(|ps| api::PlayerStateInfo {
                position_ticks: Some(ps.position_ticks),
                can_seek: ps.can_seek,
                is_paused: ps.is_paused,
                is_muted: ps.is_muted,
                volume_level: ps.volume_level,
                audio_stream_index: ps.audio_stream_index,
                subtitle_stream_index: ps.subtitle_stream_index,
                media_source_id: ps
                    .media_source_id
                    .clone(),
                play_method: ps
                    .play_method
                    .clone(),
                repeat_mode: "RepeatNone".to_string(),
                playback_order: "Default".to_string(),
            })
            .unwrap_or_default(),
        );

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
                    c.playable_media_types
                        .clone(),
                    c.supported_commands
                        .clone(),
                    c.supports_media_control,
                    c.supports_media_control,
                )
            });

        let last_paused_date = ps.and_then(|ps| ps.last_paused_at);
        let now_playing_queue: Vec<_> = ps
            .and_then(|ps| {
                ps.now_playing_queue
                    .clone()
            })
            .unwrap_or_default();
        let playlist_item_id = ps.and_then(|ps| {
            ps.playlist_item_id
                .clone()
        });

        // Populate NowPlayingQueueFullItems from queue item IDs.
        let mut now_playing_queue_full_items =
            Vec::with_capacity(now_playing_queue.len());
        for qi in &now_playing_queue {
            if let Ok(Some(m)) = db::Media::get_by_id(
                &state
                    .ctx
                    .db,
                &qi.id,
            )
            .await
            {
                now_playing_queue_full_items.push(api::db_media_to_item(m));
            }
        }

        let remote_end_point = device
            .remote_ip
            .clone();

        let user_name = device
            .user(
                &state
                    .ctx
                    .db,
            )
            .await?
            .map(|u| u.username)
            .unwrap_or_default();

        sessions.push(api::SessionInfoDto {
            id: Some(
                device
                    .id
                    .clone(),
            ),
            device_id: Some(
                device
                    .id
                    .clone(),
            ),
            device_name: Some(
                device
                    .name
                    .clone(),
            ),
            client: Some(
                device
                    .app_name
                    .clone(),
            ),
            application_version: Some(
                device
                    .app_version
                    .clone(),
            ),
            user_id: device
                .user_id
                .to_string(),
            user_name: Some(user_name),
            last_activity_date: device
                .last_activity_at
                .unwrap_or_else(Utc::now),
            last_playback_check_in: device
                .last_activity_at
                .unwrap_or_else(Utc::now),
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
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("not found")?;
    let ms = media
        .mark_played(
            &state
                .ctx
                .db,
            &session.user,
            true,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

#[delete("/userplayeditems/{id}")]
pub async fn user_unmark_played(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Path(id): Path<Uuid>,
) -> Result<impl IntoResponse> {
    let media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &id,
    )
    .await?
    .context_not_found("not found")?;
    let ms = media
        .mark_unplayed(
            &state
                .ctx
                .db,
            &session.user,
            true,
        )
        .await?;
    Ok(Json(api::db_state_to_dto(ms, &media)).into_response())
}

/// Jellyfin-compatible master HLS playlist endpoint.
/// Creates a transcode session and returns a master.m3u8 playlist.
#[get("/videos/{id}/master.m3u8")]
pub async fn master_hls_video(
    State(state): State<AppState>,
    auth: auth::AuthSession,
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
        .unwrap_or_else(|| {
            common::get_uuid()
                .as_simple()
                .to_string()
        });

    tracing::debug!("Using play session ID: {}", play_session_id);

    let encoding_opts_hls = crate::db::Settings::get_encoding_config(
        &state
            .ctx
            .db,
    )
    .await
    .unwrap_or_default();
    let video_transcode_enabled_hls = encoding_opts_hls
        .enable_video_transcoding
        .unwrap_or(true);
    let video_codec_raw = q
        .video_codec
        .as_deref()
        .unwrap_or("copy");
    let video_codec = if video_codec_raw == "copy" || !video_transcode_enabled_hls {
        "copy".to_string()
    } else {
        "h264".to_string()
    };
    let audio_codec = q
        .audio_codec
        .unwrap_or_else(|| "aac".to_string());
    let segment_length = q
        .segment_length
        .unwrap_or(6) as u32;

    // Look up existing session or create a new one.
    // When the client seeks it sends the same PlaySessionId but with a new
    // StartTimeTicks.  In that case we must stop the old transcode job and
    // restart from the requested position — otherwise the player waits for
    // segments that the old job will never produce at the new offset.
    let is_seeking = q
        .start_time_ticks
        .is_some_and(|t| t > 0);
    if is_seeking {
        if state
            .ctx
            .sessions
            .get_transcode(&play_session_id)
            .is_some()
        {
            tracing::debug!(
                play_session_id = %play_session_id,
                start_time_ticks = ?q.start_time_ticks,
                "seek detected — stopping old transcode session and restarting"
            );
            state
                .ctx
                .sessions
                .stop_transcode(&play_session_id)
                .await;
        }
    }
    let session = if let Some(existing) = state
        .ctx
        .sessions
        .get_transcode(&play_session_id)
    {
        existing
    } else {
        // Fetch media info to get the stream URL
        let media_source_id = q
            .media_source_id
            .unwrap_or(id);
        let media = db::Media::get_by_id(
            &state
                .ctx
                .db,
            &media_source_id,
        )
        .await?
        .context_not_found("media not found")?;

        let mut resolved_media = media.clone();
        if resolved_media.kind == db::MediaKind::StreamGroup {
            let gid = resolved_media.id;
            let candidates = db::StreamGroup::streams_for(
                &state
                    .ctx
                    .db,
                &gid,
                &id,
            )
            .await?;
            resolved_media = candidates
                .into_iter()
                .next()
                .context_not_found("no streams available for this group")?;
        }
        if matches!(
            resolved_media.kind,
            db::MediaKind::Movie | db::MediaKind::Episode
        ) {
            let sources = resolved_media
                .streams(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            resolved_media = if let Some(wanted) = q.media_source_id {
                sources
                    .iter()
                    .find(|s| s.id == wanted)
                    .cloned()
            } else {
                None
            }
            .or_else(|| {
                sources
                    .into_iter()
                    .next()
            })
            .context_not_found("no playable source found")?;
        } else if resolved_media.kind == db::MediaKind::Track {
            let sources = resolved_media
                .streams(
                    &state
                        .ctx
                        .db,
                )
                .await?;
            resolved_media = sources
                .into_iter()
                .next()
                .context_not_found("no stream found for track")?;
        }

        let input_url = resolved_media
            .stream_info
            .as_ref()
            .map(|si| {
                si.descriptor
                    .server_input(
                        resolved_media.id,
                        state
                            .ctx
                            .config
                            .port,
                    )
            })
            .context_not_found("media source has no URL")?;

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
            // Take the maximum of stored runtime and probe data so a stale/short
            // metadata value can't truncate the playlist for a longer file.
            let stored_ticks = resolved_media
                .runtime
                .or(media.runtime)
                .filter(|&r| r > 0)
                .and_then(|r| r.to_ticks(TickUnit::Seconds));
            let probe_ticks = resolved_media
                .probe_data
                .as_ref()
                .and_then(|p| p.run_time_ticks)
                .filter(|&t| t > 0);
            let rt = match (stored_ticks, probe_ticks) {
                (Some(a), Some(b)) => Some(a.max(b)),
                (a, b) => a.or(b),
            };
            match rt {
                Some(t) if t > 0 => t,
                _ => db::Media::get_by_id(
                    &state
                        .ctx
                        .db,
                    &id,
                )
                .await
                .ok()
                .flatten()
                .and_then(|m| m.runtime)
                .filter(|&r| r > 0)
                .and_then(|r| r.to_ticks(TickUnit::Seconds))
                .unwrap_or(0),
            }
        };
        debug!(runtime_ticks, is_live, segment_length, "transcode session");
        let source_video_stream = resolved_media
            .probe_data
            .as_ref()
            .and_then(|p| p.video_stream());
        let source_video_codec = source_video_stream
            .as_ref()
            .and_then(|s| {
                s.codec
                    .clone()
            });
        let source_video_profile = source_video_stream
            .as_ref()
            .and_then(|s| {
                s.profile
                    .clone()
            });
        let source_video_level = source_video_stream
            .as_ref()
            .and_then(|s| s.level);
        let source_video_range_type = source_video_stream
            .as_ref()
            .and_then(|s| s.video_range_type);
        let source_video_width = source_video_stream
            .as_ref()
            .and_then(|s| s.width);
        let source_video_height = source_video_stream
            .as_ref()
            .and_then(|s| s.height);
        let source_frame_rate = source_video_stream
            .as_ref()
            .and_then(|s| s.real_frame_rate);
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
        let source_audio_stream = resolved_media
            .probe_data
            .as_ref()
            .and_then(|p| p.audio_stream());
        let source_audio_codec = source_audio_stream.and_then(|s| {
            s.codec
                .clone()
        });
        let session = TranscodeSession::new(
            play_session_id.clone(),
            id,
            media_source_id,
            input_url.clone(),
            output_dir,
            video_codec.clone(),
            audio_codec.clone(),
            q.audio_stream_index
                .map(|v| v as i32),
            q.subtitle_stream_index
                .map(|v| v as i32),
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
            source_audio_codec,
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
        let encoding_opts = encoding_opts_hls.clone();
        let params = crate::transcode::engine::TranscodeParams {
            input_url,
            output_dir: session
                .read()
                .await
                .output_dir
                .clone(),
            video_codec: video_codec.clone(),
            audio_codec: audio_codec.clone(),
            segment_length,
            start_time_ticks: q.start_time_ticks,
            max_width: q
                .max_width
                .map(|v| v as u32),
            max_height: q
                .max_height
                .map(|v| v as u32),
            // Prefer an explicit VideoBitRate; fall back to MaxStreamingBitrate so
            // the encoder targets the client-requested cap rather than CRF mode.
            video_bitrate: q
                .video_bit_rate
                .map(|v| v as u32)
                .or_else(|| {
                    q.max_streaming_bitrate
                        .map(|b| b as u32)
                }),
            audio_bitrate: q
                .audio_bit_rate
                .map(|v| v as u32),
            // Force stereo downmix when transcoding audio — multi-channel AAC
            // (e.g. 6.1 from DTS-HD) causes MEDIA_ERR_SRC_NOT_SUPPORTED on most
            // browsers and iOS Safari.
            audio_channels: if audio_codec == "copy" { None } else { Some(2) },
            audio_stream_index: q
                .audio_stream_index
                .map(|v| v as i32),
            subtitle_stream_index: q
                .subtitle_stream_index
                .map(|v| v as i32),
            burn_subtitle: q.subtitle_method
                == Some(api::SubtitleDeliveryMethod::Encode),
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
            vaapi_driver: encoding_opts
                .vaapi_driver
                .unwrap_or_default(),
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
            is_live,
        };

        // Spawn the transcode task with proper error handling
        let media_title_for_log = resolved_media
            .title
            .clone();
        let transcode_reasons_for_log = q
            .transcode_reasons
            .clone();
        let log_user = auth
            .user
            .username
            .clone();
        let log_client = auth
            .device
            .app_name
            .clone();
        let session_clone = session.clone();
        tokio::spawn(async move {
            let start_secs = params
                .start_time_ticks
                .unwrap_or(0)
                / 10_000_000;
            let resolution = match (params.max_width, params.max_height) {
                (Some(w), Some(h)) => format!("{}x{}", w, h),
                (Some(w), None) => format!("{}w", w),
                (None, Some(h)) => format!("{}h", h),
                _ => "native".to_string(),
            };
            info!(
                play_session_id = %play_session_id,
                title = %media_title_for_log,
                user = %log_user,
                client = %log_client,
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
    let session_read = session
        .read()
        .await;
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
        .context_not_found("PlaySessionId is required")?;

    let session = state
        .ctx
        .sessions
        .get_transcode(&play_session_id)
        .context_not_found("transcode session not found")?;

    // Keep the session alive.
    state
        .ctx
        .sessions
        .ping(&play_session_id);

    let session_read = session
        .read()
        .await;
    let is_live = session_read.is_live;
    let use_fmp4 = session_read.use_fmp4();
    let playlist_path = session_read.variant_playlist_path();
    let psid = session_read
        .id
        .clone();

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

        // For non-live fMP4 VOD: once ffmpeg finishes it appends #EXT-X-ENDLIST and the
        // playlist type stays as EVENT. Upgrade EVENT→VOD so hls.js treats the stream as
        // a completed VOD rather than a live feed; leave live streams untouched.
        let is_complete = !is_live && content.contains("#EXT-X-ENDLIST");

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
                } else if is_complete && line == "#EXT-X-PLAYLIST-TYPE:EVENT" {
                    "#EXT-X-PLAYLIST-TYPE:VOD".to_string()
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
            .and_then(|s| {
                s.rsplit('_')
                    .next()
            })
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
        .context_not_found("PlaySessionId is required")?;

    trace!(
        segment_id = %segment_id,
        play_session_id = %play_session_id,
        runtime_ticks = ?q.runtime_ticks,
        "HLS segment request"
    );

    let session = state
        .ctx
        .sessions
        .get_transcode(&play_session_id);

    // The fMP4 init segment is served at "init.mp4" — strip_segment_extension
    // reduces that to "init", so we detect it here and serve it directly.
    if segment_id == "init" {
        let init_path = match &session {
            Some(s) => s
                .read()
                .await
                .init_segment_path(),
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
            None::<()>.context_not_found("fMP4 init segment not ready")?;
        }
        state
            .ctx
            .sessions
            .ping(&play_session_id);
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
        Some(s) => s
            .read()
            .await
            .segment_path(&segment_id),
        None => state
            .ctx
            .sessions
            .segment_path(&play_session_id, &segment_id),
    };

    // Parse the requested segment index from the filename.
    let requested_idx: Option<u32> = segment_id
        .rsplit('_')
        .next()
        .and_then(|n| {
            n.parse::<u32>()
                .ok()
        });

    if let Some(ref session) = session {
        // Update playback position for the buffer monitor.
        if let Some(idx) = requested_idx {
            use std::sync::atomic::Ordering;
            let s = session
                .read()
                .await;
            let prev = s
                .last_segment_index
                .load(Ordering::Relaxed);
            if idx > prev {
                s.last_segment_index
                    .store(idx, Ordering::Relaxed);
            }
        }
    }

    // If the segment doesn't exist and we have a live session, check whether
    // FFmpeg needs to be restarted at a different position (like Jellyfin does).
    if !segment_path.exists() {
        if let (Some(session), Some(requested_idx)) = (&session, requested_idx) {
            let s = session
                .read()
                .await;
            let output_dir = s
                .output_dir
                .clone();
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
                let has_running_ffmpeg = s
                    .kill_tx
                    .is_some();
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
                    let input_url = s
                        .input_url
                        .clone();
                    let video_codec = s
                        .video_codec
                        .clone();
                    let audio_codec = s
                        .audio_codec
                        .clone();
                    let audio_stream_index = s.audio_stream_index;
                    let subtitle_stream_index = s.subtitle_stream_index;
                    let burn_subtitle = s.burn_subtitle;
                    drop(s);

                    // Kill running FFmpeg and clean up stale segments (params
                    // like bitrate/codec may change, so old segments are invalid).
                    {
                        let (kill_tx, wait_done) = {
                            let mut s = session
                                .write()
                                .await;
                            (
                                s.kill_tx
                                    .take(),
                                s.wait_done
                                    .clone(),
                            )
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
                    let start_time_ticks = q
                        .runtime_ticks
                        .unwrap_or_else(|| {
                            (requested_idx as i64 * segment_length as i64)
                                .to_ticks(TickUnit::Seconds)
                                .unwrap_or(0)
                        });

                    let encoding_opts = crate::db::Settings::get_encoding_config(
                        &state
                            .ctx
                            .db,
                    )
                    .await
                    .unwrap_or_default();
                    let params = crate::transcode::engine::TranscodeParams {
                        input_url,
                        output_dir: output_dir.clone(),
                        video_codec,
                        audio_codec: audio_codec.clone(),
                        segment_length,
                        start_time_ticks: Some(start_time_ticks),
                        max_width: q
                            .max_width
                            .map(|v| v as u32),
                        max_height: q
                            .max_height
                            .map(|v| v as u32),
                        video_bitrate: q
                            .video_bit_rate
                            .map(|v| v as u32)
                            .or_else(|| {
                                q.max_streaming_bitrate
                                    .map(|b| b as u32)
                            }),
                        audio_bitrate: q
                            .audio_bit_rate
                            .map(|v| v as u32),
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
                        vaapi_driver: encoding_opts
                            .vaapi_driver
                            .unwrap_or_default(),
                        source_video_range_type: session
                            .read()
                            .await
                            .source_video_range_type,
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
                        is_live: false,
                    };

                    // Reinitialise the session's state for the new transcode run.
                    {
                        let mut s = session
                            .write()
                            .await;
                        s.state = TranscodeState::Starting;
                        let _ = s
                            .state_tx
                            .send(TranscodeState::Starting);
                        s.start_time_secs = (start_time_ticks / 10_000_000) as u32;
                        s.playback_offset_secs
                            .store(
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
            None::<()>.context_not_found(&format!(
                "transcode session {} gone and segment {} not on disk",
                play_session_id, segment_id
            ))?;
        }
        None::<()>.context_not_found(&format!(
            "segment {} not ready after timeout",
            segment_id
        ))?;
    }

    // Keep the session alive — the segment request counts as activity.
    state
        .ctx
        .sessions
        .ping(&play_session_id);

    let file = tokio::fs::File::open(&segment_path).await?;
    let stream = ReaderStream::new(file);
    let body = Body::from_stream(stream);

    // fMP4 segments (.m4s) use video/mp4; MPEG-TS segments use video/mp2t.
    let content_type = if segment_path
        .extension()
        .and_then(|e| e.to_str())
        == Some("m4s")
    {
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

// ── Session remote control ──────────────────────────────────────────────────

#[api_query]
#[derive(Default)]
struct RemotePlayQuery {
    item_ids: remux_sdks::CommaSeparatedList<Uuid>,
    play_command: Option<String>,
    start_position_ticks: Option<i64>,
    media_source_id: Option<String>,
    audio_stream_index: Option<i32>,
    subtitle_stream_index: Option<i32>,
    start_index: Option<i32>,
}

#[api_query]
#[derive(Default)]
struct RemotePlaystateQuery {
    seek_position_ticks: Option<i64>,
    controlling_user_id: Option<String>,
}

#[api_query]
#[derive(Default)]
struct RemoteViewingQuery {
    item_type: Option<String>,
    item_id: Option<String>,
    item_name: Option<String>,
}

#[api_query]
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RemoteFullCommand {
    name: Option<String>,
    controlling_user_id: Option<String>,
    arguments: Option<std::collections::HashMap<String, String>>,
}

#[api_query]
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RemoteMessageBody {
    header: Option<String>,
    text: Option<String>,
    timeout_ms: Option<i64>,
}

/// Instruct a session to play a list of items.
#[post("/sessions/{sessionid}/playing")]
pub async fn remote_play(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(session_id): Path<String>,
    Query(q): Query<RemotePlayQuery>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({
        "ItemIds": *q.item_ids,
        "PlayCommand": q.play_command.unwrap_or_else(|| "PlayNow".to_string()),
        "StartPositionTicks": q.start_position_ticks.unwrap_or(0),
        "MediaSourceId": q.media_source_id,
        "AudioStreamIndex": q.audio_stream_index,
        "SubtitleStreamIndex": q.subtitle_stream_index,
        "StartIndex": q.start_index,
    });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemotePlay {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Send a playstate command (pause/stop/seek/next/prev) to a session.
#[post("/sessions/{sessionid}/playing/{command}")]
pub async fn remote_playstate_command(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((session_id, command)): Path<(String, String)>,
    Query(q): Query<RemotePlaystateQuery>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({
        "Command": command,
        "SeekPositionTicks": q.seek_position_ticks,
        "ControllingUserId": q.controlling_user_id,
    });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemotePlaystate {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Send a named general command (e.g. VolumeUp) to a session.
#[post("/sessions/{sessionid}/command/{command}")]
pub async fn remote_general_command(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((session_id, command)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({ "Name": command, "Arguments": {} });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemoteCommand {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Send a full general command object to a session.
#[post("/sessions/{sessionid}/command")]
pub async fn remote_full_command(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(session_id): Path<String>,
    Json(body): Json<RemoteFullCommand>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({
        "Name": body.name,
        "ControllingUserId": body.controlling_user_id,
        "Arguments": body.arguments.unwrap_or_default(),
    });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemoteCommand {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Send a system command to a session (forwarded as a GeneralCommand).
#[post("/sessions/{sessionid}/system/{command}")]
pub async fn remote_system_command(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path((session_id, command)): Path<(String, String)>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({ "Name": command, "Arguments": {} });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemoteCommand {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Display a message on a session.
#[post("/sessions/{sessionid}/message")]
pub async fn remote_message(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(session_id): Path<String>,
    Json(body): Json<RemoteMessageBody>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({
        "Name": "DisplayMessage",
        "Arguments": {
            "Header": body.header.unwrap_or_default(),
            "Text": body.text.unwrap_or_default(),
            "TimeoutMs": body.timeout_ms,
        },
    });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemoteCommand {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Instruct a session to browse to an item.
#[post("/sessions/{sessionid}/viewing")]
pub async fn remote_viewing(
    State(state): State<AppState>,
    _session: auth::AuthSession,
    Path(session_id): Path<String>,
    Query(q): Query<RemoteViewingQuery>,
) -> Result<impl IntoResponse> {
    let target = state
        .ctx
        .sessions
        .get(&session_id)
        .context_not_found("session not found")?;
    let data = serde_json::json!({
        "Name": "DisplayContent",
        "Arguments": {
            "ItemType": q.item_type.unwrap_or_default(),
            "ItemId": q.item_id.unwrap_or_default(),
            "ItemName": q.item_name.unwrap_or_default(),
        },
    });
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::RemoteCommand {
            device_id: target.device_id,
            data,
        });
    Ok(StatusCode::NO_CONTENT)
}

/// Report that this client is currently viewing an item.
#[post("/sessions/viewing")]
pub async fn report_viewing(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

/// Add an additional user to a session.
#[post("/sessions/{sessionid}/user/{userid}")]
pub async fn add_session_user(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
    Path((_session_id, _user_id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

/// Remove an additional user from a session.
#[delete("/sessions/{sessionid}/user/{userid}")]
pub async fn remove_session_user(
    State(_state): State<AppState>,
    _session: auth::AuthSession,
    Path((_session_id, _user_id)): Path<(String, Uuid)>,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
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
        state
            .ctx
            .sessions
            .stop_transcode(&play_session_id)
            .await;
        let _ = state
            .ctx
            .ws_tx
            .send(crate::ws::WsEvent::SessionsChanged);
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
        .refresh_streams(&mut media, &state.ctx)
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

#[api_query]
pub struct BitrateTestQuery {
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

    let mut media = db::Media::get_by_id(
        &state
            .ctx
            .db,
        &media_source_id,
    )
    .await?
    .context_not_found("media source not found")?;

    if matches!(
        media.kind,
        db::MediaKind::Movie | db::MediaKind::Episode | db::MediaKind::Track
    ) {
        media = media
            .streams(
                &state
                    .ctx
                    .db,
            )
            .await?
            .get(0)
            .context_not_found("no sources found")?
            .clone();
    }

    let url = media
        .stream_info
        .as_ref()
        .map(|si| {
            si.descriptor
                .server_input(
                    media.id,
                    state
                        .ctx
                        .config
                        .port,
                )
        })
        .context_not_found("media source has no URL")?;

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

    if !output
        .status
        .success()
    {
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
    if input
        .trim_start()
        .starts_with("WEBVTT")
    {
        return input.to_string();
    }
    let mut out = String::from("WEBVTT\n\n");
    for block in input
        .trim()
        .split("\n\n")
    {
        let lines: Vec<&str> = block
            .lines()
            .collect();
        if lines.len() < 2 {
            continue;
        }
        // Skip the sequence number line (all digits), keep timecodes + text
        let rest = if lines[0]
            .trim()
            .chars()
            .all(|c| c.is_ascii_digit())
        {
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
    let lang = lang
        .trim()
        .to_lowercase();
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
    let sub_file = sub_url
        .rsplit('/')
        .next()
        .unwrap_or(sub_url);
    let sub_tok = tokens(sub_file);
    let mut src_tok = tokens(
        source_name
            .as_deref()
            .unwrap_or(""),
    );
    src_tok.extend(tokens(
        source_path
            .as_deref()
            .unwrap_or(""),
    ));
    sub_tok
        .intersection(&src_tok)
        .count() as i32
}

/// Inject external subtitles into a list of `MediaSourceInfo` entries.
pub(super) async fn inject_external_subtitles(
    ctx: &crate::AppContext,
    subtitle_media: &crate::db::Media,
    media_sources: &mut Vec<api::MediaSourceInfo>,
    item_id: Uuid,
    api_key: &str,
    sub_langs: Vec<String>,
) {
    let subs = ctx
        .addons
        .fetch_subtitles(subtitle_media, &ctx.db)
        .await;
    if subs.is_empty() {
        return;
    }

    let filtered: Vec<_> = if sub_langs.is_empty() {
        subs
    } else {
        subs.into_iter()
            .filter(|s| {
                let two = s
                    .lang
                    .as_deref()
                    .and_then(lang_to_two_letter);
                two.map_or(false, |two| {
                    sub_langs
                        .iter()
                        .any(|p| two.eq_ignore_ascii_case(p.trim()))
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
                let two = s
                    .lang
                    .as_deref()
                    .and_then(lang_to_two_letter);
                sub_langs
                    .iter()
                    .position(|p| {
                        two.as_deref()
                            .map_or(false, |t| t.eq_ignore_ascii_case(p.trim()))
                    })
                    .unwrap_or(usize::MAX)
            };
            rank(a)
                .cmp(&rank(b))
                .then(sb.cmp(sa))
        });

        let mut lang_counts: std::collections::HashMap<String, usize> =
            std::collections::HashMap::new();
        let scored: Vec<_> = scored
            .into_iter()
            .filter(|(_, s)| {
                let key = s
                    .lang
                    .clone()
                    .unwrap_or_else(|| "und".to_string());
                let count = lang_counts
                    .entry(key)
                    .or_insert(0);
                if *count < 2 {
                    *count += 1;
                    true
                } else {
                    false
                }
            })
            .collect();

        let wants_default = !sub_langs.is_empty()
            && source
                .default_subtitle_stream_index
                .is_none();
        for (i, (_, sub)) in scored
            .iter()
            .enumerate()
        {
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
            source
                .media_streams
                .push(stream);
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
        .map(|c| {
            c.0.clone()
        })
        .unwrap_or_default();

    // Load saved stream selections (best-effort; failure means no recall)
    let resolved_media = crate::db::Media::get_by_id(db, media_id)
        .await
        .ok()
        .flatten();

    let saved_audio: Option<i64>;
    let saved_subtitle: Option<i64>;

    if let Some(media) = resolved_media {
        match sqlx::query_as::<_, crate::db::UserMediaState>(
            "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(user.id)
        .bind(media.id)
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
                let exists = source
                    .media_streams
                    .iter()
                    .any(|s| {
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
                let exists = source
                    .media_streams
                    .iter()
                    .any(|s| {
                        s.index == idx
                            && matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                    });
                if exists {
                    // Clear any previous default flag, set the recalled one
                    for s in source
                        .media_streams
                        .iter_mut()
                    {
                        if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                            s.is_default = Some(false);
                        }
                    }
                    source.default_subtitle_stream_index = Some(idx);
                    if let Some(s) = source
                        .media_streams
                        .iter_mut()
                        .find(|s| s.index == idx)
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
        if source
            .default_subtitle_stream_index
            .is_none()
        {
            if let Some(ref pref) = cfg.subtitle_language_preference {
                let pref_two = lang_to_two_letter(pref);
                if let Some(ref target) = pref_two {
                    if let Some(stream) = source
                        .media_streams
                        .iter_mut()
                        .find(|s| {
                            matches!(s.type_, Some(api::MediaStreamType::Subtitle))
                                && s.language
                                    .as_deref()
                                    .and_then(lang_to_two_letter)
                                    .as_deref()
                                    == Some(target.as_str())
                        })
                    {
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
        for s in source
            .media_streams
            .iter_mut()
        {
            if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = None;
    };

    let set_default = |source: &mut api::MediaSourceInfo, idx: Option<i64>| {
        for s in source
            .media_streams
            .iter_mut()
        {
            if matches!(s.type_, Some(api::MediaStreamType::Subtitle)) {
                s.is_default = Some(false);
            }
        }
        source.default_subtitle_stream_index = idx;
        if let Some(i) = idx {
            if let Some(s) = source
                .media_streams
                .iter_mut()
                .find(|s| s.index == i)
            {
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
            if source
                .default_subtitle_stream_index
                .is_none()
            {
                let idx = source
                    .media_streams
                    .iter()
                    .find_map(|s| {
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
            let forced_idx = source
                .media_streams
                .iter()
                .find_map(|s| {
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
                    .and_then(|s| {
                        s.language
                            .clone()
                    });

                let sub_lang = source
                    .media_streams
                    .iter()
                    .find(|s| s.index == def_idx)
                    .and_then(|s| {
                        s.language
                            .clone()
                    });

                let audio_two = audio_lang
                    .as_deref()
                    .and_then(lang_to_two_letter);
                let sub_two = sub_lang
                    .as_deref()
                    .and_then(lang_to_two_letter);

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
