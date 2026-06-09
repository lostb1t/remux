use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, WebSocket},
    },
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

use crate::{AppState, api, common::get_uuid, db, db::auth::AuthSession};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct SessionMessageType(pub i32);

impl SessionMessageType {
    pub const FORCE_KEEP_ALIVE: Self = Self(0);
    pub const GENERAL_COMMAND: Self = Self(1);
    pub const SESSIONS: Self = Self(3);
    pub const PLAY: Self = Self(4);
    pub const PLAYSTATE: Self = Self(7);
    pub const LIBRARY_CHANGED: Self = Self(11);
    pub const USER_DELETED: Self = Self(12);
    pub const USER_UPDATED: Self = Self(13);
    pub const SESSIONS_START: Self = Self(29);
    pub const SESSIONS_STOP: Self = Self(30);
    pub const KEEP_ALIVE: Self = Self(33);
}

#[derive(Serialize)]
#[serde(rename_all = "PascalCase")]
struct OutboundMessage<T: Serialize> {
    message_type: SessionMessageType,
    message_id: Uuid,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InboundMessage {
    message_type: SessionMessageType,
    data: Option<serde_json::Value>,
}

#[derive(Debug, Clone)]
pub enum WsEvent {
    UserUpdated(Uuid),
    UserDeleted(Uuid),
    LibraryChanged,
    SessionsChanged,
    RemotePlay {
        device_id: String,
        data: serde_json::Value,
    },
    RemotePlaystate {
        device_id: String,
        data: serde_json::Value,
    },
    RemoteCommand {
        device_id: String,
        data: serde_json::Value,
    },
}

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    session: AuthSession,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state, session))
}

async fn handle_socket(mut socket: WebSocket, state: AppState, session: AuthSession) {
    let my_device_id = session
        .device
        .id
        .clone();
    let mut event_rx = state
        .ctx
        .ws_tx
        .subscribe();
    let mut sessions_deadline: Option<Instant> = None;
    let mut sessions_interval_ms: u64 = 10_000;

    loop {
        // Copy so the async block can capture without holding a borrow.
        let tick_at = sessions_deadline;

        tokio::select! {
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Text(text))) => {
                        if let Ok(inbound) = serde_json::from_str::<InboundMessage>(&text) {
                            match inbound.message_type {
                                SessionMessageType::KEEP_ALIVE => {
                                    if !send_msg::<()>(&mut socket, SessionMessageType::KEEP_ALIVE, None).await {
                                        return;
                                    }
                                }
                                SessionMessageType::SESSIONS_START => {
                                    let (initial_ms, interval_ms) = parse_sessions_data(inbound.data.as_ref());
                                    sessions_interval_ms = interval_ms;
                                    sessions_deadline = Some(Instant::now() + Duration::from_millis(initial_ms));
                                }
                                SessionMessageType::SESSIONS_STOP => {
                                    sessions_deadline = None;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => return,
                    _ => {}
                }
            }

            _ = async {
                match tick_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let sessions = build_sessions(&state).await;
                if !send_msg(&mut socket, SessionMessageType::SESSIONS, Some(sessions)).await {
                    return;
                }
                sessions_deadline = Some(Instant::now() + Duration::from_millis(sessions_interval_ms));
            }

            result = event_rx.recv() => {
                match result {
                    Ok(WsEvent::UserUpdated(user_id)) => {
                        if let Ok(Some(user)) = db::User::get_by_id(&state.ctx.db, &user_id).await {
                            if !send_msg(&mut socket, SessionMessageType::USER_UPDATED, Some(api::db_user_to_dto(&state.ctx.config.data_dir, user))).await {
                                return;
                            }
                        }
                    }
                    Ok(WsEvent::UserDeleted(user_id)) => {
                        if !send_msg(&mut socket, SessionMessageType::USER_DELETED, Some(user_id.to_string())).await {
                            return;
                        }
                    }
                    Ok(WsEvent::LibraryChanged) => {
                        if !send_msg::<()>(&mut socket, SessionMessageType::LIBRARY_CHANGED, None).await {
                            return;
                        }
                    }
                    Ok(WsEvent::SessionsChanged) => {
                        let sessions = build_sessions(&state).await;
                        if !send_msg(&mut socket, SessionMessageType::SESSIONS, Some(sessions)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlay { device_id, data }) if device_id == my_device_id => {
                        if !send_msg(&mut socket, SessionMessageType::PLAY, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlaystate { device_id, data }) if device_id == my_device_id => {
                        if !send_msg(&mut socket, SessionMessageType::PLAYSTATE, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemoteCommand { device_id, data }) if device_id == my_device_id => {
                        if !send_msg(&mut socket, SessionMessageType::GENERAL_COMMAND, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlay { .. } | WsEvent::RemotePlaystate { .. } | WsEvent::RemoteCommand { .. }) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

async fn send_msg<T: Serialize>(
    socket: &mut WebSocket,
    message_type: SessionMessageType,
    data: Option<T>,
) -> bool {
    let msg = OutboundMessage {
        message_type,
        message_id: get_uuid(),
        data,
    };
    match serde_json::to_string(&msg) {
        Ok(json) => socket
            .send(Message::Text(json.into()))
            .await
            .is_ok(),
        Err(_) => false,
    }
}

/// Parse "initialMs,intervalMs" from SessionsStart data.
/// Falls back to (0, 10_000) if parsing fails.
fn parse_sessions_data(data: Option<&serde_json::Value>) -> (u64, u64) {
    let s = data
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let mut parts = s.splitn(2, ',');
    let initial = parts
        .next()
        .and_then(|v| {
            v.trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(0);
    let interval = parts
        .next()
        .and_then(|v| {
            v.trim()
                .parse::<u64>()
                .ok()
        })
        .unwrap_or(10_000);
    (initial, interval)
}

async fn build_sessions(state: &AppState) -> Vec<api::SessionInfoDto> {
    let devices: std::collections::HashMap<String, db::auth::Device> =
        db::auth::Device::get_all(
            &state
                .ctx
                .db,
            None,
        )
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.id.clone(), d))
        .collect();

    state
        .ctx
        .sessions
        .get_all()
        .into_iter()
        .map(|session| {
            let transcoding_info = session
                .transcode
                .as_ref()
                .and_then(|ts| {
                    ts.try_read()
                        .ok()
                })
                .map(|ts| api::TranscodingInfo {
                    audio_codec: Some(
                        ts.audio_codec
                            .clone(),
                    ),
                    video_codec: Some(
                        ts.video_codec
                            .clone(),
                    ),
                    container: Some("ts".to_string()),
                    is_video_direct: ts.video_codec == "copy",
                    is_audio_direct: ts.audio_codec == "copy",
                    transcode_reasons: ts
                        .transcode_reasons
                        .clone(),
                    ..Default::default()
                });

            let play_state = api::PlayerStateInfo {
                position_ticks: Some(session.position_ticks),
                can_seek: session.can_seek,
                is_paused: session.is_paused,
                is_muted: session.is_muted,
                volume_level: session.volume_level,
                audio_stream_index: session.audio_stream_index,
                subtitle_stream_index: session.subtitle_stream_index,
                media_source_id: session
                    .media_source_id
                    .clone(),
                play_method: session
                    .play_method
                    .clone(),
                repeat_mode: "RepeatNone".to_string(),
                playback_order: "Default".to_string(),
            };

            let device = devices.get(&session.device_id);
            let capabilities = device.and_then(|d| d.parsed_capabilities());
            // Prefer persisted device metadata when present; fall back to
            // transient playback-session values for compatibility.
            let device_name = device
                .map(|d| {
                    d.name
                        .clone()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    session
                        .device_id
                        .clone()
                });
            let client_name = device
                .map(|d| {
                    d.app_name
                        .clone()
                })
                .filter(|v| !v.is_empty())
                .unwrap_or_else(|| {
                    session
                        .client_name
                        .clone()
                });
            let application_version = device
                .map(|d| {
                    d.app_version
                        .clone()
                })
                .filter(|v| !v.is_empty());

            let (
                playable_media_types,
                supported_commands,
                supports_media_control,
                supports_remote_control,
            ) = capabilities
                .as_ref()
                .map_or((vec![], vec![], true, true), |c| {
                    (
                        c.playable_media_types
                            .clone(),
                        c.supported_commands
                            .clone(),
                        c.supports_media_control,
                        c.supports_media_control,
                    )
                });

            api::SessionInfoDto {
                id: Some(
                    session
                        .play_session_id
                        .clone(),
                ),
                device_id: Some(
                    session
                        .device_id
                        .clone(),
                ),
                device_name: Some(device_name),
                client: Some(client_name),
                application_version,
                user_id: session
                    .user_id
                    .to_string(),
                last_activity_date: session.last_activity,
                last_playback_check_in: session.last_activity,
                last_paused_date: session.last_paused_at,
                play_state: Some(play_state),
                capabilities,
                playable_media_types,
                supported_commands,
                supports_media_control,
                supports_remote_control,
                now_playing_item: Some(api::BaseItemDto {
                    id: session.item_id,
                    ..Default::default()
                }),
                now_playing_queue: session
                    .now_playing_queue
                    .clone()
                    .unwrap_or_default(),
                playlist_item_id: session
                    .playlist_item_id
                    .clone(),
                transcoding_info,
                is_active: true,
                server_id: crate::common::server_id(),
                ..Default::default()
            }
        })
        .collect()
}
