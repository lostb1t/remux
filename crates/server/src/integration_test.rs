use anyhow::Result;
use axum_test::TestServer;
use chrono::Utc;
use http::header::HeaderValue;
use serde_json::json;

use crate::{AppContext, Config, db, init_app_with_ctx};

pub const AUTH_HEADER: &str = "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\"";

pub fn auth_header_with_token(token: &str) -> String {
    format!(
        "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\", Token=\"{}\"",
        token
    )
}

/// Creates a test server with an in-memory SQLite DB, seeds an admin user
/// "test"/"test", and returns the server alongside the `AppContext` (which
/// carries the `SqlitePool`) so callers can insert fixture data directly.
pub async fn new_test_server() -> Result<(TestServer, AppContext)> {
    let config = Config {
        db_url: "sqlite::memory:".into(),
        ..Default::default()
    };

    let (app, ctx) = init_app_with_ctx(config).await?;

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

    Ok((server, ctx))
}

/// Spins up a test server and authenticates as the seeded "test" user.
/// Returns `(server, ctx, access_token)`.
pub async fn authenticated_server() -> (TestServer, AppContext, String) {
    let (server, ctx) = new_test_server().await.unwrap();

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
    (server, ctx, token)
}

/// Inserts a minimal `MediaKind::Source` item and returns it.
/// Since the URL is not a real stream, probe_in_place fails gracefully —
/// the endpoint still returns a valid response with source.bitrate == None.
pub async fn insert_test_source(ctx: &AppContext) -> db::Media {
    let now = Utc::now().naive_utc();
    let mut media = db::Media {
        title: "Test Source".to_string(),
        kind: db::MediaKind::Source,
        url: Some("http://test.invalid/video.mp4".to_string()),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    media.save(&ctx.db).await.expect("insert_test_source failed");
    media
}
