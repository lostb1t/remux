extern crate codspeed_divan_compat as divan;

use axum_test::TestServer;
use chrono::NaiveDateTime;
use http::header;
use remux_server::{AppContext, Config, db, init_app_with_ctx};
use serde_json::json;
use std::sync::OnceLock;
use uuid::Uuid;

fn main() {
    divan::main();
}

// ── shared fixture (seeded once for all bench variants) ───────────────────────

const SEASONS: i64 = 24;
const EPISODES_PER_SEASON: i64 = 24;
const TOTAL_SERIES: usize = 500; // largest needed by any bench

const BENCH_AUTH_HEADER: &str = r#"MediaBrowser Client="Bench", Device="Bench", DeviceId="bench-device", Version="1.0.0""#;

struct Fixture {
    server: TestServer,
    token: String,
    rt: tokio::runtime::Runtime,
    _ctx: AppContext,
}

// SAFETY: TestServer uses Arc internally and bench variants run sequentially.
unsafe impl Sync for Fixture {}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();

        let (server, ctx, token) = rt.block_on(async {
            let config = Config {
                database_url: Some("sqlite::memory:".into()),
                disable_dht: true,
                torrent_http_port: None,
                ..Default::default()
            };
            let (app, ctx) = init_app_with_ctx(config)
                .await
                .unwrap();

            let server = TestServer::builder()
                .save_cookies()
                .mock_transport()
                .build(app)
                .unwrap();

            server
                .post("/startup/user")
                .json(&json!({ "Name": "bench", "Password": "bench" }))
                .await;
            server
                .post("/startup/complete")
                .await;

            let resp = server
                .post("/users/authenticatebyname")
                .add_header(header::AUTHORIZATION, BENCH_AUTH_HEADER)
                .json(&json!({ "Username": "bench", "Pw": "bench" }))
                .await;
            let body: serde_json::Value = resp.json();
            let token = body["AccessToken"]
                .as_str()
                .unwrap()
                .to_string();
            let user_id = Uuid::parse_str(
                body["User"]["Id"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();

            let now = chrono::Utc::now().naive_utc();
            let old_ts = now - chrono::Duration::days(60);

            for i in 0..TOTAL_SERIES {
                // First 250 get old timestamps (for date-cutoff bench),
                // the rest spread across the last 90 days.
                let ts = if i < 250 {
                    old_ts
                } else {
                    now - chrono::Duration::days(i as i64 % 90)
                };
                seed_series(&ctx.db, user_id, i, ts).await;
            }

            (server, ctx, token)
        });

        Fixture {
            server,
            token,
            rt,
            _ctx: ctx,
        }
    })
}

fn auth_header(token: &str) -> String {
    format!("MediaBrowser Token=\"{token}\"")
}

// ── seeding ───────────────────────────────────────────────────────────────────

/// Upserts one series (1 + 24 seasons + 576 episodes) and its user watch state.
///
/// Watch profile (`idx % 4`):
/// - 0: S1E1 only
/// - 1: S1+S2 fully, S3E1–8
/// - 2: S1–S12 fully, S13E1–12
/// - 3: S1–S23 fully, S24E6 in-progress
async fn seed_series(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    idx: usize,
    played_at: NaiveDateTime,
) {
    let series_imdb = db::NonEmptyString::try_new(format!("tt{idx:07}")).unwrap();
    let profile = idx % 4;

    let mut items: Vec<db::Media> =
        Vec::with_capacity(1 + (SEASONS * (1 + EPISODES_PER_SEASON)) as usize);
    let mut state_rows: Vec<(Uuid, bool)> = Vec::new();

    let series_id = Uuid::from(&db::MediaIdRaw {
        kind: db::MediaKind::Series,
        external_ids: db::ExternalIds {
            imdb: Some(series_imdb.clone()),
            ..Default::default()
        },
        season: None,
        episode: None,
    });
    items.push(db::Media {
        id: series_id,
        title: format!("Bench Series {idx}"),
        kind: db::MediaKind::Series,
        external_ids: db::ExternalIds {
            imdb: Some(series_imdb.clone()),
            ..Default::default()
        },
        ..Default::default()
    });

    for s in 1..=SEASONS {
        let season_id = Uuid::from(&db::MediaIdRaw {
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            season: Some(s),
            episode: None,
        });
        items.push(db::Media {
            id: season_id,
            title: format!("Bench Series {idx} Season {s}"),
            kind: db::MediaKind::Season,
            external_ids: db::ExternalIds {
                series_imdb: Some(series_imdb.clone()),
                ..Default::default()
            },
            grandparent_id: Some(series_id),
            parent_id: Some(series_id),
            idx: Some(s),
            parent_idx: Some(s),
            ..Default::default()
        });

        for ep in 1..=EPISODES_PER_SEASON {
            let ep_id = Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                season: Some(s),
                episode: Some(ep),
            });
            items.push(db::Media {
                id: ep_id,
                title: format!("Bench Series {idx} S{s:02}E{ep:02}"),
                kind: db::MediaKind::Episode,
                external_ids: db::ExternalIds {
                    series_imdb: Some(series_imdb.clone()),
                    ..Default::default()
                },
                grandparent_id: Some(series_id),
                parent_id: Some(season_id),
                parent_idx: Some(s),
                idx: Some(ep),
                ..Default::default()
            });

            let should_play = match profile {
                0 => s == 1 && ep == 1,
                1 => s <= 2 || (s == 3 && ep <= 8),
                2 => s <= 12 || (s == 13 && ep <= 12),
                _ => s <= 23,
            };
            if should_play || (profile == 3 && s == 24 && ep == 6) {
                state_rows.push((ep_id, should_play));
            }
        }
    }

    db::Media::upsert(db, &items)
        .await
        .unwrap();

    let mut tx = db
        .begin()
        .await
        .unwrap();
    for (ep_id, is_played) in state_rows {
        if is_played {
            sqlx::query(
                "INSERT OR IGNORE INTO user_media_state \
                 (user_id, media_id, media_raw, stream_id, favorite, play_count, \
                  played_at, playback_position, last_played_at, subtitle_idx, audio_idx) \
                 VALUES (?, ?, NULL, NULL, 0, 1, ?, 0, ?, NULL, NULL)",
            )
            .bind(user_id)
            .bind(ep_id)
            .bind(played_at)
            .bind(played_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        } else {
            sqlx::query(
                "INSERT OR IGNORE INTO user_media_state \
                 (user_id, media_id, media_raw, stream_id, favorite, play_count, \
                  played_at, playback_position, last_played_at, subtitle_idx, audio_idx) \
                 VALUES (?, ?, NULL, NULL, 0, 0, NULL, 300, ?, NULL, NULL)",
            )
            .bind(user_id)
            .bind(ep_id)
            .bind(played_at)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit()
        .await
        .unwrap();
}

// ── benchmarks ───────────────────────────────────────────────────────────────

/// Scale: how the endpoint performs as the active-series result set grows.
/// All variants share the same 500-series DB; only the Limit param changes.
#[divan::bench(args = [50, 200, 500])]
fn nextup_all_scale(bencher: divan::Bencher, limit: usize) {
    let f = fixture();
    let url = format!("/shows/nextup?limit={limit}");
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.server
                .get(&url)
                .add_header(header::AUTHORIZATION, auth.as_str())
                .await;
        })
    });
}

/// EnableResumable on vs off: 25% of the 500 series (profile 3) have an
/// in-progress episode, so toggling this flag exercises a real branch.
#[divan::bench(args = [true, false])]
fn nextup_all_resumable(bencher: divan::Bencher, enable: bool) {
    let f = fixture();
    let url = format!("/shows/nextup?limit=200&enable_resumable={enable}");
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.server
                .get(&url)
                .add_header(header::AUTHORIZATION, auth.as_str())
                .await;
        })
    });
}

/// NextUpDateCutoff: epoch (all 500 series) vs 30-day cutoff (only the 250
/// recent series pass the HAVING filter).
#[divan::bench(args = ["epoch", "30days"])]
fn nextup_all_date_cutoff(bencher: divan::Bencher, cutoff: &str) {
    let f = fixture();
    let cutoff_param = match cutoff {
        "30days" => {
            let ts = chrono::Utc::now() - chrono::Duration::days(30);
            urlencoding::encode(
                &ts.format("%Y-%m-%dT%H:%M:%SZ")
                    .to_string(),
            )
            .into_owned()
        }
        _ => "1970-01-01%2000%3A00%3A00".to_string(),
    };
    let url = format!("/shows/nextup?limit=500&next_up_date_cutoff={cutoff_param}");
    let auth = auth_header(&f.token);

    bencher.bench(|| {
        f.rt.block_on(async {
            f.server
                .get(&url)
                .add_header(header::AUTHORIZATION, auth.as_str())
                .await;
        })
    });
}
