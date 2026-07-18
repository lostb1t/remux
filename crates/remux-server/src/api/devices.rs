use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{delete, get, post, query};
use uuid::Uuid;

use crate::{AppState, OptionExt, api, db::auth};
use axum_anyhow::ApiResult as Result;

#[query]
struct DeleteDeviceQuery {
    id: String,
}

#[delete("/devices")]
pub async fn delete_device(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<DeleteDeviceQuery>,
) -> Result<StatusCode> {
    auth::Device::delete_by_id(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?;
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
    session: auth::AuthSession,
    Query(params): Query<GetDevicesQuery>,
) -> Result<impl IntoResponse> {
    // Get all devices from the database
    let devices = if let Some(user_id) = params.user_id {
        auth::Device::get_by_user_id(
            &state
                .ctx
                .db,
            &user_id,
        )
        .await?
    } else {
        auth::Device::get_all(
            &state
                .ctx
                .db,
            None,
        )
        .await?
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

/// Query parameter carrying a device id (`?id=`), shared by the info/options
/// endpoints the admin Devices page uses.
#[query]
pub struct DeviceIdQuery {
    pub id: String,
}

/// `GET /Devices/Info?id=` — full info for a single device.
#[get("/devices/info")]
pub async fn get_device_info(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<DeviceIdQuery>,
) -> Result<impl IntoResponse> {
    let device = auth::Device::get_by_id(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?
    .context_not_found("Device not found")?;

    Ok(Json(api::device_info_from(&device)))
}

/// `GET /Devices/Options?id=` — the operator-editable options for a device.
#[get("/devices/options")]
pub async fn get_device_options(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<DeviceIdQuery>,
) -> Result<impl IntoResponse> {
    let device = auth::Device::get_by_id(
        &state
            .ctx
            .db,
        &q.id,
    )
    .await?
    .context_not_found("Device not found")?;

    Ok(Json(api::DeviceOptions {
        id: Some(0),
        device_id: Some(device.id),
        custom_name: device.custom_name,
    }))
}

/// `POST /Devices/Options?id=` — set (or clear) a device's custom display name.
#[post("/devices/options")]
pub async fn update_device_options(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<DeviceIdQuery>,
    Json(options): Json<api::DeviceOptions>,
) -> Result<StatusCode> {
    // Treat an empty string the same as clearing the custom name.
    let custom_name = options
        .custom_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty());

    let updated = auth::Device::set_custom_name(
        &state
            .ctx
            .db,
        &q.id,
        custom_name,
    )
    .await?;

    if updated {
        Ok(StatusCode::NO_CONTENT)
    } else {
        Ok(StatusCode::NOT_FOUND)
    }
}

#[cfg(test)]
mod tests {
    use crate::integration_test::{auth_header_with_token, authenticated_server};
    use http::header::{AUTHORIZATION, HeaderValue};

    /// A device is created on login; the admin can read it, rename it via
    /// `/devices/options`, see the custom name reflected in `/devices/info`, and
    /// delete it.
    #[tokio::test]
    async fn device_options_round_trip() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = || HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();

        // The login in `authenticated_server` used DeviceId="test-device".
        let device_id = "test-device";

        // It shows up in the device list.
        let list = server
            .get("/devices")
            .add_header(AUTHORIZATION, auth())
            .await;
        list.assert_status_ok();
        list.assert_json_contains(&serde_json::json!({
            "Items": [ { "Id": device_id } ]
        }));

        // Rename it.
        let resp = server
            .post("/devices/options")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .json(&serde_json::json!({ "CustomName": "Living Room TV" }))
            .await;
        assert_eq!(resp.status_code(), http::StatusCode::NO_CONTENT);

        // The custom name is reflected in both options and info.
        let opts = server
            .get("/devices/options")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .await;
        opts.assert_status_ok();
        opts.assert_json_contains(&serde_json::json!({
            "DeviceId": device_id,
            "CustomName": "Living Room TV"
        }));

        let info = server
            .get("/devices/info")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .await;
        info.assert_status_ok();
        info.assert_json_contains(&serde_json::json!({
            "Id": device_id,
            "CustomName": "Living Room TV"
        }));

        // Clearing it (empty string) resets to no custom name. With
        // skip_serializing_none the field is then omitted entirely.
        server
            .post("/devices/options")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .json(&serde_json::json!({ "CustomName": "" }))
            .await;
        let opts = server
            .get("/devices/options")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .await;
        opts.assert_json(&serde_json::json!({ "Id": 0, "DeviceId": device_id }));

        // Delete removes it.
        let del = server
            .delete("/devices")
            .add_query_param("id", device_id)
            .add_header(AUTHORIZATION, auth())
            .await;
        assert_eq!(del.status_code(), http::StatusCode::NO_CONTENT);
    }

    #[tokio::test]
    async fn device_info_requires_admin() {
        let (server, _guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let resp = server
            .get("/devices/info")
            .add_query_param("id", "whatever")
            .expect_failure()
            .await;
        assert_eq!(resp.status_code(), http::StatusCode::UNAUTHORIZED);
    }
}
