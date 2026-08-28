use anyhow::Result;
use axum_test::TestServer;
use chrono::Utc;
use http::header::HeaderValue;
use remux_sdks::remux::{
    MediaSourceInfo, MediaStream, MediaStreamType, VideoContainer,
};
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
        let ctx = self
            .0
            .clone();
        // Fire-and-forget shutdown: releases sockets so the next test (or a
        // server restart) can bind the same ports without "address in use" errors.
        tokio::spawn(async move {
            ctx.shutdown()
                .await;
        });
    }
}

/// Creates a test server from the given config, seeds an admin user "test"/"test",
/// and returns the server alongside a [`TestGuard`].
pub async fn new_test_server_with_config(
    config: Config,
) -> Result<(TestServer, TestGuard)> {
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

    server
        .post("/startup/complete")
        .await;

    Ok((server, TestGuard(ctx)))
}

/// Creates a test server with an in-memory SQLite DB, seeds an admin user
/// "test"/"test", and returns the server alongside a [`TestGuard`] (which
/// carries the `AppContext` and shuts down background services on drop).
pub async fn new_test_server() -> Result<(TestServer, TestGuard)> {
    new_test_server_with_config(Config {
        database_url: Some("sqlite::memory:".into()),
        torrent_http_port: None, // OS picks a free ephemeral port
        disable_dht: true,       // no DHT needed in tests; avoids socket conflicts
        ..Default::default()
    })
    .await
}

/// Spins up a test server and authenticates as the seeded "test" user.
/// Returns `(server, guard, access_token)`.
pub async fn authenticated_server() -> (TestServer, TestGuard, String) {
    let (server, guard) = new_test_server()
        .await
        .unwrap();

    let resp = server
        .post("/users/authenticatebyname")
        .add_header(
            http::header::AUTHORIZATION,
            HeaderValue::from_static(AUTH_HEADER),
        )
        .json(&json!({ "Username": "test", "Pw": "test" }))
        .await;

    let body: serde_json::Value = resp.json();
    let token = body["AccessToken"]
        .as_str()
        .unwrap()
        .to_string();
    (server, guard, token)
}

/// Inserts a test video source with pre-populated probe data (container="mp4",
/// bitrate=8_000_000, 1920×1080 h264). No ffprobe or network needed — the
/// fields are set directly so playbackinfo tests behave identically in CI and
/// locally.
pub async fn insert_test_source(ctx: &AppContext) -> db::Media {
    insert_test_source_of_kind(ctx, db::MediaKind::Stream).await
}

/// [`insert_test_source`] with an external subtitle stream, which is delivered
/// by URL rather than embedded.
pub async fn insert_test_source_with_external_subtitle(ctx: &AppContext) -> db::Media {
    insert_source(
        ctx,
        db::MediaKind::Stream,
        vec![MediaStream {
            codec: Some("subrip".to_string()),
            type_: Some(MediaStreamType::Subtitle),
            index: 2,
            is_external: true,
            language: Some("eng".to_string()),
            ..Default::default()
        }],
    )
    .await
}

/// [`insert_test_source`] for a specific kind. Playbackinfo branches on it:
/// `TvChannel` is live, `Track` goes down the HLS-only path.
pub async fn insert_test_source_of_kind(
    ctx: &AppContext,
    kind: db::MediaKind,
) -> db::Media {
    insert_source(ctx, kind, vec![]).await
}

async fn insert_source(
    ctx: &AppContext,
    kind: db::MediaKind,
    extra_streams: Vec<MediaStream>,
) -> db::Media {
    let now = Utc::now().naive_utc();

    // Build minimal probe_data so playbackinfo can make transcode decisions
    // without needing ffprobe or a live network connection.
    let probe = MediaSourceInfo {
        id: Uuid::new_v4(),
        container: Some(VideoContainer::Mp4),
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
        ]
        .into_iter()
        .chain(extra_streams)
        .collect(),
        ..Default::default()
    };

    // Tracks are rejected without a music provider id.
    let external_ids = if kind == db::MediaKind::Track {
        db::ExternalIds {
            deezer_track: Some(1),
            ..Default::default()
        }
    } else {
        db::ExternalIds::default()
    };

    let mut media = db::Media {
        title: "Test Source".to_string(),
        kind,
        external_ids,
        stream_info: Some(crate::stream::StreamInfo {
            descriptor: crate::stream::StreamDescriptor::Local(
                "test-fixture.mp4".into(),
            ),
            ..Default::default()
        }),
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

/// Every `ApiKey=` a response hands back must carry the caller's real token.
/// The token is a `Secret`, so one formatting slip redacts it for every client
/// at once; that is worth checking over the whole body rather than the one
/// field a test happens to know about.
pub fn assert_api_keys_are_real(value: &serde_json::Value, token: &str) {
    fn walk(v: &serde_json::Value, token: &str, seen: &mut usize) {
        match v {
            serde_json::Value::String(s) => {
                for tail in s
                    .split("ApiKey=")
                    .skip(1)
                {
                    *seen += 1;
                    let got = tail
                        .split('&')
                        .next()
                        .unwrap_or_default();
                    assert_eq!(got, token, "ApiKey should be the caller's token: {s}");
                }
            }
            serde_json::Value::Array(items) => {
                for v in items {
                    walk(v, token, seen);
                }
            }
            serde_json::Value::Object(fields) => {
                for v in fields.values() {
                    walk(v, token, seen);
                }
            }
            _ => {}
        }
    }

    let mut seen = 0;
    walk(value, token, &mut seen);
    assert!(
        seen > 0,
        "expected the response to carry an ApiKey: {value}"
    );
}

/// Long enough that a percentage of it lands on a whole second, so a test can
/// say "stopped at 95%" without the threshold turning on a rounding.
pub const MOVIE_RUNTIME_SECONDS: i64 = 6_000;

fn stable_id(kind: db::MediaKind, external_ids: &db::ExternalIds) -> Uuid {
    Uuid::from(&db::MediaIdRaw {
        kind,
        external_ids: external_ids.clone(),
        season: None,
        episode: None,
    })
}

/// A movie carrying the ids a tracking service keys on. Shared so the ids only
/// have to agree with themselves: a test asserting on `tmdb` is otherwise
/// asserting against a literal three modules away.
pub async fn seed_movie(ctx: &AppContext) -> db::Media {
    let external_ids = db::ExternalIds {
        imdb: db::NonEmptyString::try_new("tt0113277".to_string()).ok(),
        tmdb: Some(949),
        ..Default::default()
    };
    let mut media = db::Media {
        id: stable_id(db::MediaKind::Movie, &external_ids),
        title: "Heat".into(),
        kind: db::MediaKind::Movie,
        runtime: Some(MOVIE_RUNTIME_SECONDS),
        external_ids,
        ..Default::default()
    };
    media
        .save(&ctx.db)
        .await
        .unwrap();
    media
}

/// The first episode of a series, with the season and series rows it hangs
/// off. Only the series carries ids, which is what makes it the fixture for
/// "an episode is matched through its series".
pub async fn seed_episode(ctx: &AppContext) -> db::Media {
    seed_episode_of(ctx, "tt0306414").await
}

/// As [`seed_episode`], for a series identified by `imdb`. A test needing TMDB
/// to answer differently needs a series of its own: the http cache is keyed on
/// the url and outlives any one test.
pub async fn seed_episode_of(ctx: &AppContext, imdb: &str) -> db::Media {
    seed_episode_with(
        ctx,
        db::ExternalIds {
            imdb: db::NonEmptyString::try_new(imdb.to_string()).ok(),
            tvdb: Some(79126),
            ..Default::default()
        },
    )
    .await
}

/// As [`seed_episode_of`], for a series carrying exactly `external_ids`.
pub async fn seed_episode_with(
    ctx: &AppContext,
    external_ids: db::ExternalIds,
) -> db::Media {
    let mut series = db::Media {
        id: stable_id(db::MediaKind::Series, &external_ids),
        title: "The Wire".into(),
        kind: db::MediaKind::Series,
        external_ids,
        ..Default::default()
    };
    series
        .save(&ctx.db)
        .await
        .unwrap();

    let mut season = db::Media {
        title: "Season 1".into(),
        kind: db::MediaKind::Season,
        parent_id: Some(series.id),
        grandparent_id: Some(series.id),
        idx: Some(1),
        ..Default::default()
    };
    season
        .save(&ctx.db)
        .await
        .unwrap();

    let mut episode = db::Media {
        title: "The Target".into(),
        kind: db::MediaKind::Episode,
        parent_id: Some(season.id),
        grandparent_id: Some(series.id),
        idx: Some(1),
        parent_idx: Some(1),
        ..Default::default()
    };
    episode
        .save(&ctx.db)
        .await
        .unwrap();
    episode
}

/// Stores an addon row and installs `provider` as its media-tracker
/// capability. The runtime list has to be rebuilt wholesale because the real
/// one is built from registered presets, which have no way to carry a stub.
pub async fn register_media_tracker(
    ctx: &AppContext,
    name: &str,
    provider: std::sync::Arc<dyn crate::addons::media_tracker::MediaTrackerAddon>,
) -> crate::addons::Addon {
    let now = Utc::now().naive_utc();
    let row = crate::addons::Addon {
        id: crate::common::get_uuid(),
        name: name.into(),
        preset: crate::addons::AddonPresetRef {
            kind: "scripted".into(),
            config: serde_json::Value::Null.into(),
        },
        resources: vec![],
        types: vec![],
        enabled: true,
        priority: 0,
        created_at: now,
        updated_at: now,
        system: false,
        is_default: true,
        http_redirect_stream: false,
        service_filter: vec![],
    };
    row.insert(&ctx.db)
        .await
        .unwrap();

    let mut runtimes = ctx
        .addons
        .list_for_user(&ctx.db, None)
        .await;
    runtimes.push(crate::addons::AddonRuntime {
        row: row.clone(),
        caps: crate::addons::AddonCapabilities {
            media_tracker: Some(provider),
            ..Default::default()
        },
    });
    ctx.addons
        .replace_runtimes_for_test(runtimes);
    row
}
