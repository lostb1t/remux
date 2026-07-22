use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{delete, get, query};
use uuid::Uuid;

use crate::{AppState, api, db, db::auth};
use axum_anyhow::ApiResult as Result;

#[query]
struct DeleteDeviceQuery {
    id: Option<String>,
    #[serde(rename = "userId", alias = "UserId")]
    user_id: Option<Uuid>,
}

#[delete("/devices")]
pub async fn delete_device(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Query(q): Query<DeleteDeviceQuery>,
) -> Result<StatusCode> {
    match (q.id.as_deref(), q.user_id) {
        (Some(id), _) => {
            // Look up device first to get user_id (needed for compound PK delete) and logging.
            if let Some(dev) = auth::Device::get_by_id(&state.ctx.db, id).await? {
                auth::Device::delete_by_id(&state.ctx.db, id, &dev.user_id).await?;
                let _ = state.ctx.ws_tx.send(crate::ws::WsEvent::SessionsChanged);
                let target_user =
                    db::User::get_by_id(&state.ctx.db, &dev.user_id).await?;
                db::ActivityLog::insert(
                    &state.ctx.db,
                    &session.user.id,
                    &session.user.username,
                    "session_revoked",
                    Some(&dev.user_id),
                    target_user.as_ref().map(|u| u.username.as_str()),
                    Some(id),
                    Some(&dev.name),
                    None,
                )
                .await?;
            }
        }
        (None, Some(user_id)) => {
            let target_user =
                db::User::get_by_id(&state.ctx.db, &user_id).await?;
            auth::Device::delete_all_for_user(
                &state.ctx.db,
                &user_id,
                Some(&session.device.access_token),
            )
            .await?;
            let _ = state.ctx.ws_tx.send(crate::ws::WsEvent::SessionsChanged);
            db::ActivityLog::insert(
                &state.ctx.db,
                &session.user.id,
                &session.user.username,
                "all_sessions_revoked",
                Some(&user_id),
                target_user.as_ref().map(|u| u.username.as_str()),
                None,
                None,
                None,
            )
            .await?;
        }
        (None, None) => {
            return Ok(StatusCode::BAD_REQUEST);
        }
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Query parameters for devices endpoint
#[query]
pub struct GetDevicesQuery {
    pub user_id: Option<uuid::Uuid>,
}

/// Get all devices
#[get("/devices")]
pub async fn get_devices(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Query(params): Query<GetDevicesQuery>,
) -> Result<impl IntoResponse> {
    let devices = if let Some(user_id) = params.user_id {
        auth::Device::get_by_user_id(&state.ctx.db, &user_id).await?
    } else {
        auth::Device::get_all(&state.ctx.db, None).await?
    };

    // Batch-fetch usernames so we can populate last_user_name without N queries.
    let user_ids: Vec<uuid::Uuid> = devices.iter().map(|d| d.user_id).collect();
    let users = db::User::get_by_ids(&state.ctx.db, &user_ids).await?;
    let username_map: std::collections::HashMap<uuid::Uuid, String> =
        users.into_iter().map(|u| (u.id, u.username)).collect();

    let caller_token = session.device.access_token.as_str();
    let device_infos: Vec<api::DeviceInfo> = devices
        .iter()
        .map(|device| {
            let username = username_map.get(&device.user_id).map(String::as_str);
            api::device_info_from(device, username, caller_token)
        })
        .collect();

    let result = api::QueryResult {
        items: device_infos.clone(),
        total_record_count: device_infos.len() as i64,
        start_index: 0,
        ..Default::default()
    };

    Ok(Json(result))
}
