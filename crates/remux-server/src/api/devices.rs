use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use http::StatusCode;
use remux_macros::{delete, get};
use serde::Deserialize;
use uuid::Uuid;

use crate::AppState;
use crate::api;
use crate::db::auth;
use axum_anyhow::ApiResult as Result;

#[derive(Deserialize)]
struct DeleteDeviceQuery {
    id: String,
}

#[delete("/devices")]
pub async fn delete_device(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<DeleteDeviceQuery>,
) -> Result<StatusCode> {
    auth::Device::delete_by_id(&state.ctx.db, &q.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Query parameters for devices endpoint
#[derive(Deserialize)]
pub struct GetDevicesQuery {
    #[serde(alias = "userId")]
    pub user_id: Option<uuid::Uuid>,
}

/// Get all devices
#[get("/devices")]
pub async fn get_devices(
    State(state): State<AppState>,
    session: auth::AuthSession,
    Query(params): Query<GetDevicesQuery>,
) -> Result<impl IntoResponse> {
    // Get all devices from the database
    let devices = if let Some(user_id) = params.user_id {
        auth::Device::get_by_user_id(&state.ctx.db, &user_id).await?
    } else {
        auth::Device::get_all(&state.ctx.db).await?
    };

    // Convert to Jellyfin DeviceInfo format
    let device_infos: Vec<api::DeviceInfo> = devices
        .iter()
        .map(|device| api::device_info_from(device))
        .collect();

    // Return as QueryResult format
    let result = api::QueryResult {
        items: device_infos.clone(),
        total_record_count: device_infos.len() as i64,
        start_index: 0,
        ..Default::default()
    };

    Ok(Json(result))
}
