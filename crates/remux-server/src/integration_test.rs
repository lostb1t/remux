use anyhow::Result;
use axum_test::TestServer;
use chrono::Utc;
use http::header::HeaderValue;
use remux_sdks::remux::{MediaSourceInfo, MediaStream, MediaStreamType};
use serde_json::json;
use uuid::Uuid;

use crate::{AppContext, Config, db, init_app_with_ctx};

pub const AUTH_HEADER: &str = "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\"";

pub fn auth_header_with_token(token: &str) -> String {
    format!(
        "MediaBrowser Client=\"Test\", Device=\"Test\", DeviceId=\"test-device\", Version=\"1.0.0\", Token=\"{}\"",
        token
    )
}

/// RAII guard that shuts down the `AppContext` (releases torrent/DHT sockets)
/// when the test ends. Hold this for the lifetime of the test.
pub struct TestGuard(pub AppContext);

impl Drop for TestGuard {
    fn drop(&mut self) {
        let ctx = self.0.clone();
        // Fire-and-forget shutdown: releases sockets so the next test (or a
        // server restart) can bind the same ports without "address in use" errors.
        tokio::spawn(async move {
            ctx.shutdown().await;
        });
    }
}

/// Creates a test server with an in-memory SQLite DB, seeds an admin user
/// "test"/"test", and returns the server alongside a [`TestGuard`] (which
/// carries the `AppContext` and shuts down background services on drop).
pub async fn new_test_server() -> Result<(TestServer, TestGuard)> {
    let config = Config {
        database_url: "sqlite::memory:".into(),
        torrent_http_port: None, // OS picks a free ephemeral port
        disable_dht: true,       // no DHT needed in tests; avoids socket conflicts
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

    Ok((server, TestGuard(ctx)))
}

/// Spins up a test server and authenticates as the seeded "test" user.
/// Returns `(server, guard, access_token)`.
pub async fn authenticated_server() -> (TestServer, TestGuard, String) {
    let (server, guard) = new_test_server().await.unwrap();

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
    (server, guard, token)
}

/// Inserts a test video source with pre-populated probe data (container="mp4",
/// bitrate=8_000_000, 1920×1080 h264). No ffprobe or network needed — the
/// fields are set directly so playbackinfo tests behave identically in CI and
/// locally.
pub async fn insert_test_source(ctx: &AppContext) -> db::Media {
    let now = Utc::now().naive_utc();

    // Build minimal probe_data so playbackinfo can make transcode decisions
    // without needing ffprobe or a live network connection.
    let probe = MediaSourceInfo {
        id: Uuid::new_v4(),
        container: Some("mp4".to_string()),
        bitrate: Some(8_000_000),
        run_time_ticks: Some(100_000_000),
        media_streams: vec![
            MediaStream {
                codec: Some("h264".to_string()),
                type_: Some(MediaStreamType::Video),
                index: 0,
                width: Some(1920),
                height: Some(1080),
                ..Default::default()
            },
            MediaStream {
                codec: Some("aac".to_string()),
                type_: Some(MediaStreamType::Audio),
                index: 1,
                ..Default::default()
            },
        ],
        ..Default::default()
    };

    let mut media = db::Media {
        title: "Test Source".to_string(),
        kind: db::MediaKind::Stream,
        url: Some(crate::stream::StreamDescriptor::Http(
            "https://test-videos.co.uk/vids/bigbuckbunny/mp4/h264/1080/Big_Buck_Bunny_1080_10s_5MB.mp4"
                .to_string(),
        )),
        probe_data: Some(probe),
        created_at: now,
        updated_at: now,
        ..Default::default()
    };
    media
        .save(&ctx.db)
        .await
        .expect("insert_test_source failed");
    media
}
