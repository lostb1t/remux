use axum::{
    Json,
    extract::{Query, State},
    response::IntoResponse,
};
use http::StatusCode;
use remux_macros::{delete, get, patch, query};
use uuid::Uuid;

use crate::{AppState, api, db, db::auth};
use axum_anyhow::ApiResult as Result;

#[query]
pub struct DeleteDeviceQuery {
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
                state
                    .ctx
                    .signals
                    .emit(crate::signals::Event::SessionsChanged);
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
                    session
                        .device
                        .access_token
                        .expose(),
                ),
            )
            .await?;
            state
                .ctx
                .signals
                .emit(crate::signals::Event::SessionsChanged);
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
    db::ActivityLog::rotate(
        &state
            .ctx
            .db,
        state
            .ctx
            .config
            .activity_log_retention_days,
    )
    .await
    .ok();
    Ok(StatusCode::NO_CONTENT)
}

/// Query parameters for devices endpoint
#[query]
pub struct GetDevicesQuery {
    pub user_id: Option<uuid::Uuid>,
    pub start_index: Option<i64>,
    pub limit: Option<i64>,
    #[serde(rename = "searchTerm", alias = "search_term")]
    pub search_term: Option<String>,
}

/// Get all devices
#[get("/devices")]
pub async fn get_devices(
    State(state): State<AppState>,
    session: auth::AdminSession,
    Query(params): Query<GetDevicesQuery>,
) -> Result<impl IntoResponse> {
    let db = &state
        .ctx
        .db;

    let (devices, total_record_count, start_index) =
        if let Some(user_id) = params.user_id {
            let devices = auth::Device::get_by_user_id(db, &user_id).await?;
            let count = devices.len() as i64;
            (devices, count, 0i32)
        } else {
            let offset = params
                .start_index
                .unwrap_or(0);
            let limit = params
                .limit
                .unwrap_or(50)
                .clamp(0, 200);
            let filter = params
                .search_term
                .as_deref()
                .map(|t| format!("%{}%", t.to_lowercase()));
            let (devices, total) =
                auth::Device::get_paged(db, offset, limit, filter.as_deref()).await?;
            (devices, total, offset as i32)
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
    let users = db::User::get_by_ids(db, &user_ids).await?;
    let username_map: std::collections::HashMap<uuid::Uuid, String> = users
        .into_iter()
        .map(|u| (u.id, u.username))
        .collect();

    let caller_token = session
        .device
        .access_token
        .expose()
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
        items: device_infos,
        total_record_count,
        start_index,
        ..Default::default()
    };

    Ok(Json(result))
}

/// A device id is only unique per user, so both are needed.
#[query]
pub struct PatchDeviceQuery {
    id: String,
    user_id: Uuid,
}

#[patch("/remux/devices")]
pub async fn patch_device(
    State(state): State<AppState>,
    _session: auth::AdminSession,
    Query(q): Query<PatchDeviceQuery>,
    Json(body): Json<api::PatchDeviceRequest>,
) -> Result<StatusCode> {
    if !auth::Device::set_4k_support(
        &state
            .ctx
            .db,
        q.user_id,
        &q.id,
        body.four_k,
    )
    .await?
    {
        return Ok(StatusCode::NOT_FOUND);
    }
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration_test::{auth_header_with_token, authenticated_server};
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

    #[tokio::test]
    async fn get_devices_search_filter() {
        let (server, _guard, token) = authenticated_server().await;

        // Register a second device.
        server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER_2),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;

        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();

        // Partial match on "test" username → both devices.
        let body: serde_json::Value = server
            .get("/devices")
            .add_query_param("searchTerm", "tes")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .await
            .json();
        assert_eq!(body["TotalRecordCount"], 2);

        // No match → 0 results.
        let body: serde_json::Value = server
            .get("/devices")
            .add_query_param("searchTerm", "nonexistent")
            .add_header(http::header::AUTHORIZATION, auth)
            .await
            .json();
        assert_eq!(body["TotalRecordCount"], 0);
        assert_eq!(
            body["Items"]
                .as_array()
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn get_devices_pagination() {
        let (server, _guard, token) = authenticated_server().await;

        // Register a second device.
        server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER_2),
            )
            .json(&json!({ "Username": "test", "Pw": "test" }))
            .await;

        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();

        // First page: limit=1, start=0 → 1 item, total=2.
        let body: serde_json::Value = server
            .get("/devices")
            .add_query_param("startIndex", "0")
            .add_query_param("limit", "1")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .await
            .json();
        assert_eq!(body["TotalRecordCount"], 2);
        assert_eq!(
            body["Items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );

        // Second page: limit=1, start=1 → 1 item, total=2.
        let body: serde_json::Value = server
            .get("/devices")
            .add_query_param("startIndex", "1")
            .add_query_param("limit", "1")
            .add_header(http::header::AUTHORIZATION, auth)
            .await
            .json();
        assert_eq!(body["TotalRecordCount"], 2);
        assert_eq!(
            body["Items"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    async fn user_id(server: &axum_test::TestServer, auth: &HeaderValue) -> String {
        server
            .get("/users/me")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .await
            .json::<serde_json::Value>()["Id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    async fn four_k(
        server: &axum_test::TestServer,
        auth: &HeaderValue,
    ) -> serde_json::Value {
        let body: serde_json::Value = server
            .get("/devices")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .await
            .json();
        body["Items"][0]["Remux"]["four_k"].clone()
    }

    #[tokio::test]
    async fn admin_sets_a_devices_4k_support() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();
        let user_id = user_id(&server, &auth).await;

        assert_eq!(four_k(&server, &auth).await, json!("Auto"));

        for value in ["Supported", "Unsupported", "Auto"] {
            server
                .patch("/remux/devices")
                .add_query_param("id", "test-device")
                .add_query_param("userId", &user_id)
                .add_header(http::header::AUTHORIZATION, auth.clone())
                .json(&json!({ "four_k": value }))
                .await
                .assert_status(StatusCode::NO_CONTENT);
            assert_eq!(four_k(&server, &auth).await, json!(value));
        }

        server
            .patch("/remux/devices")
            .add_query_param("id", "no-such-device")
            .add_query_param("userId", &user_id)
            .add_header(http::header::AUTHORIZATION, auth)
            .json(&json!({ "four_k": "Supported" }))
            .expect_failure()
            .await
            .assert_status(StatusCode::NOT_FOUND);
    }

    /// What playback learns shows in the list as Supported, and nothing else
    /// in it moves.
    #[tokio::test]
    async fn the_device_list_shows_learned_4k_as_supported() {
        let (server, guard, token) = authenticated_server().await;
        let ctx = &guard.0;
        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();
        let user_id: Uuid = user_id(&server, &auth)
            .await
            .parse()
            .unwrap();

        let mut before: serde_json::Value = server
            .get("/devices")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .await
            .json();
        assert_eq!(before["Items"][0]["Remux"]["four_k"], json!("Auto"));

        crate::db::auth::Device::mark_4k_capable(&ctx.db, user_id, "test-device")
            .await
            .unwrap();

        let mut after: serde_json::Value = server
            .get("/devices")
            .add_header(http::header::AUTHORIZATION, auth)
            .await
            .json();
        assert_eq!(after["Items"][0]["Remux"]["four_k"], json!("Supported"));

        // Each request moves the device's last activity, so leave that out.
        before["Items"][0]["Remux"]["four_k"] = json!("Supported");
        for body in [&mut before, &mut after] {
            body["Items"][0]
                .as_object_mut()
                .unwrap()
                .remove("DateLastActivity");
        }
        assert_eq!(after, before);
    }

    #[tokio::test]
    async fn a_non_admin_cannot_set_4k_support() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = HeaderValue::from_str(&auth_header_with_token(&token)).unwrap();
        let admin_id = user_id(&server, &auth).await;

        server
            .post("/users/new")
            .add_header(http::header::AUTHORIZATION, auth.clone())
            .json(&json!({ "Name": "regular", "Password": "pw" }))
            .await
            .assert_status_ok();
        let login = server
            .post("/users/authenticatebyname")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_static(AUTH_HEADER_2),
            )
            .json(&json!({ "Username": "regular", "Pw": "pw" }))
            .await;
        let regular = HeaderValue::from_str(&auth_header_with_token(
            login.json::<serde_json::Value>()["AccessToken"]
                .as_str()
                .unwrap(),
        ))
        .unwrap();
        let regular_id = user_id(&server, &regular).await;

        for (device_id, owner) in
            [("test-device", &admin_id), ("test-device-2", &regular_id)]
        {
            server
                .patch("/remux/devices")
                .add_query_param("id", device_id)
                .add_query_param("userId", owner)
                .add_header(http::header::AUTHORIZATION, regular.clone())
                .json(&json!({ "four_k": "Supported" }))
                .expect_failure()
                .await
                .assert_status(StatusCode::UNAUTHORIZED);
        }

        let body: serde_json::Value = server
            .get("/devices")
            .add_header(http::header::AUTHORIZATION, auth)
            .await
            .json();
        for device in body["Items"]
            .as_array()
            .unwrap()
        {
            assert_eq!(device["Remux"]["four_k"], json!("Auto"));
        }
    }
}
