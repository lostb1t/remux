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
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    AppState, api, api::session::build_session_list, common::get_uuid, db,
    db::auth::AuthSession,
};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub enum SessionMessageType {
    ForceKeepAlive,
    GeneralCommand,
    Sessions,
    Play,
    Playstate,
    LibraryChanged,
    UserDeleted,
    UserUpdated,
    SessionsStart,
    SessionsStop,
    KeepAlive,
    #[serde(other)]
    Unknown,
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
    info!(device_id = %my_device_id, "WS connection opened");
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
                                SessionMessageType::KeepAlive => {
                                    let _ = session.device.touch(&state.ctx.db, None).await;
                                    if !send_msg::<()>(&mut socket, SessionMessageType::KeepAlive, None).await {
                                        return;
                                    }
                                }
                                SessionMessageType::SessionsStart => {
                                    let (initial_ms, interval_ms) = parse_sessions_data(inbound.data.as_ref());
                                    sessions_interval_ms = interval_ms;
                                    sessions_deadline = Some(Instant::now() + Duration::from_millis(initial_ms));
                                }
                                SessionMessageType::SessionsStop => {
                                    sessions_deadline = None;
                                }
                                _ => {}
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        info!(device_id = %my_device_id, "WS connection closed");
                        return;
                    }
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
                if !send_msg(&mut socket, SessionMessageType::Sessions, Some(sessions)).await {
                    return;
                }
                sessions_deadline = Some(Instant::now() + Duration::from_millis(sessions_interval_ms));
            }

            result = event_rx.recv() => {
                match result {
                    Ok(WsEvent::UserUpdated(user_id)) => {
                        if let Ok(Some(user)) = db::User::get_by_id(&state.ctx.db, &user_id).await {
                            if !send_msg(&mut socket, SessionMessageType::UserUpdated, Some(api::db_user_to_dto(&state.ctx.config.data_dir, user))).await {
                                return;
                            }
                        }
                    }
                    Ok(WsEvent::UserDeleted(user_id)) => {
                        if !send_msg(&mut socket, SessionMessageType::UserDeleted, Some(user_id.to_string())).await {
                            return;
                        }
                    }
                    Ok(WsEvent::LibraryChanged) => {
                        if !send_msg::<()>(&mut socket, SessionMessageType::LibraryChanged, None).await {
                            return;
                        }
                    }
                    Ok(WsEvent::SessionsChanged) => {
                        let sessions = build_sessions(&state).await;
                        if !send_msg(&mut socket, SessionMessageType::Sessions, Some(sessions)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlay { device_id, data }) if device_id == my_device_id => {
                        info!(device_id = %device_id, "delivering Play to WS client");
                        if !send_msg(&mut socket, SessionMessageType::Play, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlaystate { device_id, data }) if device_id == my_device_id => {
                        info!(device_id = %device_id, "delivering Playstate to WS client");
                        if !send_msg(&mut socket, SessionMessageType::Playstate, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemoteCommand { device_id, data }) if device_id == my_device_id => {
                        info!(device_id = %device_id, "delivering GeneralCommand to WS client");
                        if !send_msg(&mut socket, SessionMessageType::GeneralCommand, Some(data)).await {
                            return;
                        }
                    }
                    Ok(WsEvent::RemotePlay { device_id, .. }) => {
                        info!(target = %device_id, me = %my_device_id, "RemotePlay not for this connection");
                    }
                    Ok(WsEvent::RemotePlaystate { .. } | WsEvent::RemoteCommand { .. }) => {}
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
    build_session_list(state, Some(Duration::from_secs(600)), None)
        .await
        .unwrap_or_default()
}
