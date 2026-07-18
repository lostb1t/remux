use axum::{
    Json,
    extract::{Path, State},
    response::IntoResponse,
};
use axum_anyhow::ApiResult as Result;
use axum_extra::extract::Query;
use chrono::Utc;
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use serde::Deserialize;
use serde_json::json;
use serde_with::{DurationSeconds, serde_as};
use std::time::Duration;
use tracing::info;
use uuid::Uuid;

use crate::{
    AppState, IntoApiError, OptionExt, ResultExt, api, common,
    common::{TickUnit, ToRunTimeTicks},
    db,
    db::auth,
    playback::session::TranscodeSession,
    services::MediaResolveService,
};

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

/// Best-effort audit record for a playback event. Looks up the item title so the
/// entry reads naturally, and never fails the calling request.
async fn record_playback(
    state: &AppState,
    session: &auth::AuthSession,
    item_id: &Uuid,
    kind: db::ActivityKind,
    verb: &str,
) {
    let title = db::Media::get_by_id(
        &state
            .ctx
            .db,
        item_id,
    )
    .await
    .ok()
    .flatten()
    .map(|m| m.title)
    .unwrap_or_else(|| "an item".to_string());

    db::ActivityLog::record_ignore(
        &state
            .ctx
            .db,
        db::NewActivity::info(
            format!(
                "{} {verb} {title}",
                session
                    .user
                    .username
            ),
            kind,
        )
        .with_user(
            session
                .user
                .id
                .to_string(),
        )
        .with_item(item_id.to_string()),
    )
    .await;
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
    record_playback(
        &state,
        &session,
        &data.item_id,
        db::ActivityKind::VideoPlayback,
        "is playing",
    )
    .await;
    let _ = state
        .ctx
        .ws_tx
        .send(crate::ws::WsEvent::SessionsChanged);
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[post("/sessions/playing/progress")]
pub async fn report_playback_progress(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Json(data): Json<api::PlaybackInfo>,
) -> Result<impl IntoResponse> {
    let effective_psid = data
        .play_session_id
        .clone()
        .or_else(|| {
            state
                .ctx
                .sessions
                .get_by_device(
                    &session
                        .device
                        .id,
                )
                .map(|s| s.play_session_id)
        });
    if let Some(ref psid) = effective_psid {
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
    let effective_psid = data
        .play_session_id
        .clone()
        .or_else(|| {
            state
                .ctx
                .sessions
                .get_by_device(
                    &session
                        .device
                        .id,
                )
                .map(|s| s.play_session_id)
        });
    if let Some(ref psid) = effective_psid {
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
        super::movies::invalidate_recommendation_cache();
        record_playback(
            &state,
            &session,
            &data.item_id,
            db::ActivityKind::VideoPlaybackStopped,
            "has finished playing",
        )
        .await;
        let _ = state
            .ctx
            .ws_tx
            .send(crate::ws::WsEvent::SessionsChanged);
    }
    Ok(StatusCode::NO_CONTENT.into_response())
}

#[query]
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
#[query]
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
        // Use get_by_filter so preload_parents runs and db_media_to_item can
        // populate series_name / season_name for episodes.
        let mut media = if let Some(ps) = ps {
            db::Media::get_by_filter(
                &state
                    .ctx
                    .db,
                &db::MediaFilter {
                    id: Some(vec![ps.item_id]),
                    ..Default::default()
                },
            )
            .await
            .ok()
            .map(|r| {
                r.records
                    .into_iter()
                    .next()
            })
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
                .map(|m| api::db_media_to_item(m.clone(), false))
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

        // Populate NowPlayingQueueFullItems from queue item IDs. Batched: this
        // runs per session in `/sessions`, and resolving the queue one id at a
        // time issued two queries per entry. `get_by_ids` has the same
        // semantics as `get_by_id`, queue order is preserved by iterating
        // `now_playing_queue`, and entries with no row are skipped exactly as
        // the `Ok(Some(_))` match skipped them before.
        let queue_ids: Vec<uuid::Uuid> = now_playing_queue
            .iter()
            .map(|qi| qi.id)
            .collect();
        let queue_media = db::Media::get_by_ids(
            &state
                .ctx
                .db,
            &queue_ids,
        )
        .await
        .unwrap_or_default();
        let mut now_playing_queue_full_items =
            Vec::with_capacity(now_playing_queue.len());
        for qi in &now_playing_queue {
            // `get`+`clone`, not `remove`: a play queue may legitimately contain
            // the same track twice, and removing on first use would drop it.
            if let Some(m) = queue_media
                .get(&qi.id)
                .cloned()
            {
                now_playing_queue_full_items.push(api::db_media_to_item(m, false));
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
    let media = MediaResolveService::resolve_item(id, &state.ctx)
        .await?
        .context_not_found("not found")?;
    let server_config = db::Settings::get_config_or_default(
        &state
            .ctx
            .db,
    )
    .await;
    let ms = media
        .mark_played(
            &state
                .ctx
                .db,
            &session.user,
            true,
            server_config.release_date_threshold(),
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
    let media = MediaResolveService::resolve_item(id, &state.ctx)
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
// ── Session remote control ──────────────────────────────────────────────────

#[query]
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

#[query]
#[derive(Default)]
struct RemotePlaystateQuery {
    seek_position_ticks: Option<i64>,
    controlling_user_id: Option<String>,
}

#[query]
#[derive(Default)]
struct RemoteViewingQuery {
    item_type: Option<String>,
    item_id: Option<String>,
    item_name: Option<String>,
}

#[query]
#[derive(Debug, Clone, Default, serde::Deserialize)]
struct RemoteFullCommand {
    name: Option<String>,
    controlling_user_id: Option<String>,
    arguments: Option<std::collections::HashMap<String, String>>,
}

#[query]
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
