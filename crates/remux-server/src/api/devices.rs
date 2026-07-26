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
            if let Some(dev) = auth::Device::get_by_id(
                &state
                    .ctx
                    .db,
                id,
            )
            .await?
            {
                auth::Device::delete_by_id(
                    &state
                        .ctx
                        .db,
                    id,
                    &dev.user_id,
                )
                .await?;
                let _ = state
                    .ctx
                    .ws_tx
                    .send(crate::ws::WsEvent::SessionsChanged);
                let target_user = db::User::get_by_id(
                    &state
                        .ctx
                        .db,
                    &dev.user_id,
                )
                .await?;
                db::ActivityLog::insert(
                    &state
                        .ctx
                        .db,
                    &session
                        .user
                        .id,
                    &session
                        .user
                        .username,
                    "session_revoked",
                    Some(&dev.user_id),
                    target_user
                        .as_ref()
                        .map(|u| {
                            u.username
                                .as_str()
                        }),
                    Some(id),
                    Some(&dev.name),
                    None,
                )
                .await?;
            }
        }
        (None, Some(user_id)) => {
            let target_user = db::User::get_by_id(
                &state
                    .ctx
                    .db,
                &user_id,
            )
            .await?;
            auth::Device::delete_all_for_user(
                &state
                    .ctx
                    .db,
                &user_id,
                Some(
                    &session
                        .device
                        .access_token,
                ),
            )
            .await?;
            let _ = state
                .ctx
                .ws_tx
                .send(crate::ws::WsEvent::SessionsChanged);
            db::ActivityLog::insert(
                &state
                    .ctx
                    .db,
                &session
                    .user
                    .id,
                &session
                    .user
                    .username,
                "all_sessions_revoked",
                Some(&user_id),
                target_user
                    .as_ref()
                    .map(|u| {
                        u.username
                            .as_str()
                    }),
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

    // Batch-fetch usernames so we can populate last_user_name without N queries.
    let user_ids: Vec<uuid::Uuid> = {
        let mut ids: Vec<uuid::Uuid> = devices
            .iter()
            .map(|d| d.user_id)
            .collect();
        ids.sort_unstable();
        ids.dedup();
        ids
    };
    let users = db::User::get_by_ids(
        &state
            .ctx
            .db,
        &user_ids,
    )
    .await?;
    let username_map: std::collections::HashMap<uuid::Uuid, String> = users
        .into_iter()
        .map(|u| (u.id, u.username))
        .collect();

    let caller_token = session
        .device
        .access_token
        .as_str();
    let device_infos: Vec<api::DeviceInfo> = devices
        .iter()
        .map(|device| {
            let username = username_map
                .get(&device.user_id)
                .map(String::as_str);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration_test::{
        AUTH_HEADER, auth_header_with_token, authenticated_server,
    };
    use http::header::HeaderValue;
    use serde_json::json;

    const AUTH_HEADER_2: &str = "MediaBrowser Client=\"Test\", Device=\"Device2\", DeviceId=\"test-device-2\", Version=\"1.0.0\"";

    #[tokio::test]
    async fn delete_device_by_id_returns_204() {
        let (server, _guard, token) = authenticated_server().await;

        // Register a second device.
        let resp = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER_2),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;
        assert!(resp.json::<serde_json::Value>()["AccessToken"].is_string());

        // Two devices should now exist.
        let list = server
            .get("/devices")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        let body: serde_json::Value = list.json();
        assert_eq!(body["TotalRecordCount"], 2);

        // Delete the second device.
        let resp = server
            .delete("/devices")
            .add_query_param("id", "test-device-2")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // Only the original device should remain.
        let list = server
            .get("/devices")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        let body: serde_json::Value = list.json();
        assert_eq!(body["TotalRecordCount"], 1);
    }

    #[tokio::test]
    async fn delete_device_by_user_id_returns_204() {
        let (server, _guard, token) = authenticated_server().await;

        // Register a second device for the same user.
        server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER_2),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;

        // Discover the user's id from /users/me.
        let me = server
            .get("/users/me")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        let user_id = me.json::<serde_json::Value>()["Id"]
            .as_str()
            .unwrap()
            .to_string();

        // Bulk-revoke all devices for this user.
        let resp = server
            .delete("/devices")
            .add_query_param("userId", &user_id)
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        resp.assert_status(StatusCode::NO_CONTENT);

        // The admin's own session token is preserved; only the second device is gone.
        let list = server
            .get("/devices")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .await;
        let body: serde_json::Value = list.json();
        assert_eq!(body["TotalRecordCount"], 1);
    }

    #[tokio::test]
    async fn delete_device_no_params_returns_400() {
        let (server, _guard, token) = authenticated_server().await;

        let resp = server
            .delete("/devices")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth_header_with_token(&token)).unwrap(),
            )
            .expect_failure()
            .await;
        resp.assert_status(StatusCode::BAD_REQUEST);
    }
}
