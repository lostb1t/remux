//! Compatibility stubs for jellyfin-web admin-dashboard endpoints that Remux has
//! no first-class feature behind.
//!
//! Remux has no plugin system, package repositories, plugin-provided
//! configuration pages, or notification services. jellyfin-web's admin dashboard
//! nonetheless calls these endpoints on load and `JSON.parse`s the result. When a
//! route is absent the request falls through to the SPA `fallback_service` and
//! returns `index.html` (HTTP 200, `text/html`), which makes those admin pages
//! throw. These handlers return correctly-shaped **empty** Jellyfin responses so
//! the pages render (empty but functional) instead of crashing.
//!
//! Everything here is deliberately read-only/no-op: there is nothing to manage.
//! If Remux ever grows a real plugin/notification subsystem these should be
//! replaced by handlers backed by it.

use axum::{Json, extract::Path, response::IntoResponse};
use http::StatusCode;
use remux_macros::{get, post};
use serde_json::json;

use crate::db::auth;
use axum_anyhow::ApiResult as Result;

/// `GET /Plugins` — installed plugins (`PluginInfo[]`). Remux has none.
#[get("/plugins")]
pub async fn plugins(_session: auth::AdminSession) -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /Packages` — available packages from repositories (`PackageInfo[]`).
#[get("/packages")]
pub async fn packages(_session: auth::AdminSession) -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /Repositories` — configured plugin repositories (`RepositoryInfo[]`).
#[get("/repositories")]
pub async fn repositories(_session: auth::AdminSession) -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `POST /Repositories` — save repository list. No-op: Remux has no plugin system,
/// but jellyfin-web's save button must not 404.
#[post("/repositories")]
pub async fn set_repositories(
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /web/ConfigurationPages` — plugin-provided dashboard pages
/// (`ConfigurationPageInfo[]`). This is the path the current `@jellyfin/sdk`
/// uses. Remux ships none.
#[get("/web/configurationpages")]
pub async fn web_configuration_pages() -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /Dashboard/ConfigurationPages` — legacy path for the same list, still
/// called by older jellyfin-web bundles.
#[get("/dashboard/configurationpages")]
pub async fn dashboard_configuration_pages() -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /web/ConfigurationPage?name=` — a single plugin config page. None exist.
#[get("/web/configurationpage")]
pub async fn web_configuration_page() -> Result<impl IntoResponse> {
    Ok(StatusCode::NOT_FOUND)
}

/// `GET /Notifications/Types` — notification categories (`NotificationTypeInfo[]`).
#[get("/notifications/types")]
pub async fn notification_types(
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /Notifications/Services` — configured notification services
/// (`NameIdPair[]`). Remux has none.
#[get("/notifications/services")]
pub async fn notification_services(
    _session: auth::AdminSession,
) -> Result<impl IntoResponse> {
    Ok(Json(json!([])))
}

/// `GET /Notifications/{userId}` — a user's notifications (`NotificationResultDto`).
#[get("/notifications/{user_id}")]
pub async fn notifications(
    _session: auth::AuthSession,
    Path(_user_id): Path<String>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "Notifications": [],
        "TotalRecordCount": 0
    })))
}

/// `GET /Notifications/{userId}/Summary` — unread summary
/// (`NotificationsSummaryDto`).
#[get("/notifications/{user_id}/summary")]
pub async fn notifications_summary(
    _session: auth::AuthSession,
    Path(_user_id): Path<String>,
) -> Result<impl IntoResponse> {
    Ok(Json(json!({
        "UnreadCount": 0,
        "MaxUnreadNotificationLevel": null
    })))
}

#[cfg(test)]
mod tests {
    use crate::integration_test::{
        auth_header_with_token, authenticated_server, new_test_server,
    };
    use http::{StatusCode, header::HeaderValue};

    #[tokio::test]
    async fn plugin_endpoints_require_auth_and_return_empty() {
        let (server, _guard) = new_test_server()
            .await
            .unwrap();

        // Unauthenticated → 401 (NOT 404/HTML from the SPA fallback).
        for path in [
            "/plugins",
            "/packages",
            "/repositories",
            "/notifications/types",
        ] {
            let resp = server
                .get(path)
                .expect_failure()
                .await;
            assert_eq!(
                resp.status_code(),
                StatusCode::UNAUTHORIZED,
                "{path} should be 401 unauthenticated"
            );
        }
    }

    #[tokio::test]
    async fn plugin_endpoints_return_empty_arrays_when_authed() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        for path in [
            "/plugins",
            "/packages",
            "/repositories",
            "/notifications/types",
        ] {
            let resp = server
                .get(path)
                .add_header(
                    http::header::AUTHORIZATION,
                    HeaderValue::from_str(&auth).unwrap(),
                )
                .await;
            resp.assert_status_ok();
            resp.assert_json(&serde_json::json!([]));
        }
    }

    #[tokio::test]
    async fn configuration_pages_are_public_empty_arrays() {
        let (server, _guard) = new_test_server()
            .await
            .unwrap();

        for path in ["/web/configurationpages", "/dashboard/configurationpages"] {
            let resp = server
                .get(path)
                .await;
            resp.assert_status_ok();
            resp.assert_json(&serde_json::json!([]));
        }
    }

    #[tokio::test]
    async fn per_user_notifications_are_empty() {
        let (server, _guard, token) = authenticated_server().await;
        let auth = auth_header_with_token(&token);

        let resp = server
            .get("/notifications/some-user-id")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        resp.assert_json(&serde_json::json!({
            "Notifications": [],
            "TotalRecordCount": 0
        }));

        let resp = server
            .get("/notifications/some-user-id/summary")
            .add_header(
                http::header::AUTHORIZATION,
                HeaderValue::from_str(&auth).unwrap(),
            )
            .await;
        resp.assert_status_ok();
        resp.assert_json(&serde_json::json!({
            "UnreadCount": 0,
            "MaxUnreadNotificationLevel": null
        }));
    }
}
