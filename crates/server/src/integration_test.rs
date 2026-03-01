use anyhow::Result;
use axum_test::TestServer;
use http::header::HeaderValue;
use serde_json::json;

use crate::{Config, init_app_with_config};

pub const AUTH_HEADER: &str = "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\"";

pub fn auth_header_with_token(token: &str) -> String {
    format!(
        "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\", Token=\"{}\"",
        token
    )
}

/// Creates a test server with an in-memory SQLite DB and seeds an admin user
/// "test" / "test" via the startup wizard endpoints.
pub async fn new_test_server() -> Result<TestServer> {
    let config = Config {
        db_url: "sqlite::memory:".into(),
        ..Default::default()
    };

    let app = init_app_with_config(config).await?;

    let server = TestServer::builder()
        .save_cookies()
        .expect_success_by_default()
        .mock_transport()
        .build(app)?;

    // Seed admin user via startup wizard (no auth required)
    server
        .post("/startup/user")
        .json(&json!({ "Name": "test", "Password": "test" }))
        .await;

    server.post("/startup/complete").await;

    Ok(server)
}

/// Spins up a test server and authenticates as the seeded "test" user.
/// Returns `(server, access_token)`.
pub async fn authenticated_server() -> (TestServer, String) {
    let server = new_test_server().await.unwrap();

    let resp = server
        .post("/users/authenticatebyname")
        .add_header(
            http::header::AUTHORIZATION,
            HeaderValue::from_static(AUTH_HEADER),
        )
        .json(&json!({ "Username": "test", "Pw": "test" }))
        .await;

    let body: serde_json::Value = resp.json();
    let token = body["AccessToken"].as_str().unwrap().to_string();
    (server, token)
}
