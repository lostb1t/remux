use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};
use std::time::Duration;
use tokio::time::Instant;
use uuid::Uuid;

use crate::AppState;
use crate::db;
use crate::db::auth::AuthSession;
use crate::jellyfin;
use crate::playback_session::PlaybackSession;
use crate::utils::get_uuid;

// ---------------------------------------------------------------------------
// Message type constants
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(transparent)]
pub struct SessionMessageType(pub i32);

impl SessionMessageType {
    pub const FORCE_KEEP_ALIVE: Self = Self(0);
    pub const SESSIONS: Self = Self(3);
    pub const LIBRARY_CHANGED: Self = Self(11);
    pub const USER_DELETED: Self = Self(12);
    pub const USER_UPDATED: Self = Self(13);
    pub const SESSIONS_START: Self = Self(29);
    pub const SESSIONS_STOP: Self = Self(30);
    pub const KEEP_ALIVE: Self = Self(33);
}

// ---------------------------------------------------------------------------
// Wire message types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Broadcast event enum
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum WsEvent {
    UserUpdated(Uuid),
    UserDeleted(Uuid),
    LibraryChanged,
}

// ---------------------------------------------------------------------------
// Route handler
// ---------------------------------------------------------------------------

pub async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    session: AuthSession,
) -> impl IntoResponse {
    ws.on_upgrade(|socket| handle_socket(socket, state, session))
}

// ---------------------------------------------------------------------------
// Connection loop
// ---------------------------------------------------------------------------

async fn handle_socket(mut socket: WebSocket, state: AppState, _session: AuthSession) {
    let mut event_rx = state.ctx.ws_tx.subscribe();
    let mut sessions_deadline: Option<Instant> = None;
    let mut sessions_interval_ms: u64 = 10_000;

    loop {
        // Copy so the async block can capture without holding a borrow.
        let tick_at = sessions_deadline;

        tokio::select! {
            // ---- Incoming frame from client ----
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

            // ---- Sessions ticker ----
            _ = async {
                match tick_at {
                    Some(at) => tokio::time::sleep_until(at).await,
                    None => std::future::pending::<()>().await,
                }
            } => {
                let sessions = build_sessions(&state);
                if !send_msg(&mut socket, SessionMessageType::SESSIONS, Some(sessions)).await {
                    return;
                }
                sessions_deadline = Some(Instant::now() + Duration::from_millis(sessions_interval_ms));
            }

            // ---- Broadcast events ----
            result = event_rx.recv() => {
                match result {
                    Ok(WsEvent::UserUpdated(user_id)) => {
                        if let Ok(Some(user)) = db::User::get_by_id(&state.ctx.db, &user_id).await {
                            if !send_msg(&mut socket, SessionMessageType::USER_UPDATED, Some(jellyfin::db_user_to_dto(user))).await {
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
                        if !send_msg(&mut socket, SessionMessageType::LIBRARY_CHANGED, Some(serde_json::json!({
                            "FoldersAddedTo": [],
                            "FoldersRemovedFrom": [],
                            "ItemsAdded": [],
                            "ItemsRemoved": [],
                            "ItemsUpdated": []
                        }))).await {
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

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
        Ok(json) => socket.send(Message::Text(json.into())).await.is_ok(),
        Err(_) => false,
    }
}

/// Parse "initialMs,intervalMs" from SessionsStart data.
/// Falls back to (0, 10_000) if parsing fails.
fn parse_sessions_data(data: Option<&serde_json::Value>) -> (u64, u64) {
    let s = data.and_then(|v| v.as_str()).unwrap_or("");
    let mut parts = s.splitn(2, ',');
    let initial = parts
        .next()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(0);
    let interval = parts
        .next()
        .and_then(|v| v.trim().parse::<u64>().ok())
        .unwrap_or(10_000);
    (initial, interval)
}

fn build_sessions(state: &AppState) -> Vec<jellyfin::SessionInfoDto> {
    PlaybackSession::get_all(&state.ctx.store)
        .into_iter()
        .map(|session| {
            let transcoding_info = state
                .ctx
                .transcode
                .get(&session.play_session_id)
                .and_then(|ts| {
                    ts.try_read().ok().map(|ts| jellyfin::TranscodingInfo {
                        audio_codec: Some(ts.audio_codec.clone()),
                        video_codec: Some(ts.video_codec.clone()),
                        container: Some("ts".to_string()),
                        is_video_direct: ts.video_codec == "copy",
                        is_audio_direct: ts.audio_codec == "copy",
                        transcode_reasons: ts.transcode_reasons.0,
                        ..Default::default()
                    })
                });

            jellyfin::SessionInfoDto {
                id: Some(session.play_session_id.clone()),
                device_id: Some(session.device_id.clone()),
                device_name: Some(session.device_id.clone()),
                client: Some(session.client_name.clone()),
                user_id: session.user_id.to_string(),
                last_activity_date: session.last_activity,
                last_playback_check_in: session.last_activity,
                transcoding_info,
                is_active: true,
                supports_media_control: true,
                supports_remote_control: true,
                server_id: crate::utils::server_id(),
                ..Default::default()
            }
        })
        .collect()
}
