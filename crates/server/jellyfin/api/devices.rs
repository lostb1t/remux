use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use remux_macros::get;
use uuid::Uuid;

use crate::AppState;
use crate::db::auth;
use crate::jellyfin;
use axum_anyhow::ApiResult as Result;

/// Query parameters for devices endpoint
#[derive(serde::Deserialize)]
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
    let device_infos: Vec<jellyfin::DeviceInfo> = devices
        .iter()
        .map(|device| jellyfin::device_info_from(device))
        .collect();

    // Return as QueryResult format
    let result = jellyfin::QueryResult {
        items: device_infos.clone(),
        total_record_count: device_infos.len() as i64,
        start_index: 0,
        ..Default::default()
    };

    Ok(Json(result))
}
