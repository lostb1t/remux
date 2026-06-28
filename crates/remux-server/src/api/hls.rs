use axum::{
    body::Body,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use http::{Response, StatusCode};
use remux_macros::get;
use tokio_util::io::ReaderStream;
use tracing::{debug, error, info, trace, warn};
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common,
    common::{TickUnit, ToRunTimeTicks},
    db,
    db::auth,
    transcode::session::{TranscodeSession, TranscodeState},
};

/// Serializes the lookup-or-create-transcode sequence per play_session_id so
/// two racing requests for the same session can't each spawn their own
/// ffmpeg process.
static TRANSCODE_CREATE_LOCKS: crate::keyed_lock::KeyedLock<String> =
    crate::keyed_lock::KeyedLock::new();

#[get("/videos/{id}/master.m3u8")]
pub async fn master_hls_video(
    State(state): State<AppState>,
    auth: auth::AuthSession,
    Path(id): Path<Uuid>,
    Query(q): Query<api::HlsVideoQuery>,
) -> Result<impl IntoResponse> {
    debug!("master_hls_video: item_id={}, q={:?}", id, q);

    // Add debugging info for crash diagnosis
    debug!(
        "Starting HLS session setup for item {} with session ID: {:?}",
        id, q.play_session_id
    );

    let play_session_id = q
        .play_session_id
        .unwrap_or_else(|| {
            common::get_uuid()
                .as_simple()
                .to_string()
        });

    debug!("Using play session ID: {}", play_session_id);

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
    //
    // Serialize the whole stop/lookup/create/attach sequence per
    // play_session_id: the lookup-or-create path below awaits DB queries and
    // filesystem ops with no lock held, so two requests racing for the same
    // session would otherwise both see no existing transcode and each spawn
    // their own ffmpeg process, with the loser's session silently overwritten
    // (and its ffmpeg process orphaned) by attach_transcode.
    let _create_guard = TRANSCODE_CREATE_LOCKS
        .lock(play_session_id.clone())
        .await;
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
            debug!(
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
        let burn_subtitle =
            q.subtitle_method == Some(api::SubtitleDeliveryMethod::Encode);
        let text_subtitle_si = if burn_subtitle {
            q.subtitle_stream_index
                .and_then(|sub_idx| {
                    resolved_media
                        .probe_data
                        .as_ref()
                        .and_then(|probe| {
                            let stream = probe
                                .media_streams
                                .iter()
                                .find(|s| s.index == sub_idx as i64)?;
                            let is_text = stream.is_text_subtitle_stream
                                || matches!(
                                    stream
                                        .codec
                                        .as_deref()
                                        .unwrap_or(""),
                                    "subrip"
                                        | "srt"
                                        | "ass"
                                        | "ssa"
                                        | "webvtt"
                                        | "mov_text"
                                        | "text"
                                );
                            if !is_text {
                                return None;
                            }
                            let mut sub_indexes: Vec<i64> = probe
                                .media_streams
                                .iter()
                                .filter(|s| {
                                    matches!(
                                        s.type_,
                                        Some(crate::api::MediaStreamType::Subtitle)
                                    )
                                })
                                .map(|s| s.index)
                                .collect();
                            sub_indexes.sort_unstable();
                            let ordinal = sub_indexes
                                .iter()
                                .position(|&i| i == sub_idx as i64)?;
                            Some(ordinal as i64)
                        })
                })
        } else {
            None
        };
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
            burn_subtitle,
            text_subtitle_si,
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
            video_bitrate: source_video_stream
                .and_then(|s| s.bit_rate)
                .map(|b| {
                    let source = b as u32;
                    let target = q
                        .video_bit_rate
                        .map_or(source, |v| source.min(v as u32));
                    q.max_streaming_bitrate
                        .map_or(target, |c| target.min(c as u32))
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
            burn_subtitle,
            text_subtitle_si,
            subtitle_width: None,
            subtitle_height: None,
            encoding_preset: encoding_opts.encoding_preset,
            source_video_codec: session
                .read()
                .await
                .source_video_codec
                .clone(),
            source_audio_codec: session
                .read()
                .await
                .source_audio_codec
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
                error!("Transcode failed: {:#}", e);
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

fn should_serve_ffmpeg_variant_playlist(
    is_live: bool,
    use_fmp4: bool,
    start_time_secs: u32,
) -> bool {
    is_live || use_fmp4
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

    // For live streams, fMP4 sessions, and resumed TS-HLS sessions we must
    // serve the ffmpeg-written playlist:
    // - live streams need the rolling EVENT playlist
    // - fMP4 segments snap to keyframe boundaries, so actual durations differ
    //   from the target and the playlist must reflect the real segment timing
    // - resumed TS-HLS sessions start ffmpeg at a non-zero segment number; a
    //   synthetic zero-based playlist would point clients at segment_00000 even
    //   though ffmpeg is writing segment_{start_number}.ts
    if should_serve_ffmpeg_variant_playlist(
        is_live,
        use_fmp4,
        session_read.start_time_secs,
    ) {
        drop(session_read);
        // For live streams, serve the ffmpeg-written EVENT playlist directly.
        // For fMP4 VOD, also use ffmpeg's playlist because fMP4 segments snap to
        // keyframe boundaries so actual durations differ from our target.
        // For resumed TS-HLS sessions, ffmpeg's playlist carries the correct
        // non-zero MEDIA-SEQUENCE and segment filenames after -start_number.
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

        // For non-live VOD sessions: once ffmpeg finishes it appends
        // #EXT-X-ENDLIST and the playlist type stays as EVENT. Upgrade
        // EVENT→VOD so hls.js treats the stream as a completed VOD rather than
        // a live feed; leave live streams untouched.
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

#[cfg(test)]
mod tests {
    #[test]
    fn resumed_ts_hls_uses_ffmpeg_variant_playlist() {
        assert!(!super::should_serve_ffmpeg_variant_playlist(
            false, false, 0
        ));
        assert!(!super::should_serve_ffmpeg_variant_playlist(
            false, false, 1
        ));
        assert!(super::should_serve_ffmpeg_variant_playlist(false, true, 0));
        assert!(super::should_serve_ffmpeg_variant_playlist(true, false, 0));
    }
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
                    let text_subtitle_si = s.text_subtitle_si;
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
                            .map(|v| v as u32),
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
                        text_subtitle_si,
                        subtitle_width: None,
                        subtitle_height: None,
                        encoding_preset: encoding_opts.encoding_preset,
                        source_video_codec: session
                            .read()
                            .await
                            .source_video_codec
                            .clone(),
                        source_audio_codec: session
                            .read()
                            .await
                            .source_audio_codec
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
                            error!("Transcode restart failed: {:#}", e);
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
