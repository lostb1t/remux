use axum::Json;
use axum::extract::State;
use axum::response::IntoResponse;
use remux_macros::{get, route};
use http::StatusCode;
use serde_json::json;

use crate::AppState;
use crate::jellyfin;
use crate::utils::server_id;
use axum_anyhow::ApiResult as Result;

use super::{mock_items, stub};

/// TODO: make a real server id
#[get("/system/info/public")]
pub async fn system_info_public(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::PublicSystemInfo {
        local_address: Some("".to_string()),
        server_name: Some("Remux".to_string()),
        product_name: Some("Jellyfin Server".to_string()),
        startup_wizard_completed: Some(true),
        version: Some("10.10.7".to_string()),
        operating_system: Some("".to_string()),
        id: Some(server_id()),
        ..Default::default()
    }))
}

#[get("/system/info")]
pub async fn system_info(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(jellyfin::SystemInfo {
        id: Some(server_id()),
        server_name: Some(server_id()),
        // server_id: Some(server_id()),
        ..Default::default()
    }))
}

#[get("/system/ping")]
pub async fn system_ping(State(state): State<AppState>) -> Result<impl IntoResponse> {
    Ok(Json(json!("Remux Server")))
}

#[cfg(test)]
#[tokio::test]
async fn system_ping_test() {
    let server = crate::integration_test::new_test_server().await.unwrap();

    let response = server.get("/system/ping").await;

    response.assert_status_ok();
    //response.assert_text("Remux Server");
}

#[get("/system/endpoint")]
pub async fn system_endpoint(
    State(state): State<AppState>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "IsLocal": false,
        "IsInNetwork": false,

    })))
}

#[get("/syncplay/list")]
pub async fn syncplay_list(State(state): State<AppState>) -> Result<impl IntoResponse> {
    mock_items(State(state)).await
}

#[route("/quickconnect/enabled", method = "GET", method = "POST")]
pub async fn quickconnect_enabled(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}

#[route("/branding/configuration", method = "GET", method = "POST")]
pub async fn branding_configuration(State(state): State<AppState>) -> Result<impl IntoResponse> {
    stub(State(state)).await
}
