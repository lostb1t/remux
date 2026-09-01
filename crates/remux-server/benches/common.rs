use remux_server::{AppContext, Config, db, init_app_with_ctx};
use serde_json::json;
use std::sync::OnceLock;
use uuid::Uuid;

//
// 50 000 series, 1 season × 24 episodes each → ~1 250 000 rows.
// 10 000 user_media_state rows:
//   Series   0– 4 999 : S1E1 played,      timestamp = 60 days ago  (old)
//   Series 5 000– 7 499: S1E1 played,      timestamp = 7–90 days    (recent)
//   Series 7 500– 9 999: S1E2 in-progress, timestamp = 7–90 days    (recent)
//   Series 10 000–49 999: no state

const TOTAL_SERIES: usize = 50_000;
const ACTIVE_SERIES: usize = 10_000;
const OLD_ACTIVE: usize = 5_000;
const IN_PROGRESS_START: usize = 7_500;
const EPISODES: i64 = 24;

//
// 10 000 movies, created_at spread over the last 730 days.
// 2 000 in-progress (playback_position = 300, play_count = 0)
// 1 000 played      (play_count = 1)

const TOTAL_MOVIES: usize = 10_000;
const MOVIES_IN_PROGRESS: usize = 2_000;
const MOVIES_PLAYED: usize = 1_000;

const BENCH_AUTH_HEADER: &str = r#"MediaBrowser Client="Bench", Device="Bench", DeviceId="bench-device", Version="1.0.0""#;

pub struct Fixture {
    pub client: reqwest::Client,
    pub base_url: String,
    pub token: String,
    pub rt: tokio::runtime::Runtime,
    pub _ctx: AppContext,
}

// SAFETY: all fields are Send+Sync (Client/String use Arc, AppContext uses Arc).
unsafe impl Sync for Fixture {}

static FIXTURE: OnceLock<Fixture> = OnceLock::new();

pub fn fixture() -> &'static Fixture {
    FIXTURE.get_or_init(|| {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();

        let (client, ctx, token, base_url) = rt.block_on(async {
            let config = Config {
                database_url: Some("sqlite::memory:".into()),
                disable_dht: true,
                torrent_http_port: None,
                ..Default::default()
            };
            let (router, ctx) = init_app_with_ctx(config)
                .await
                .unwrap();

            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .unwrap();
            let port = listener
                .local_addr()
                .unwrap()
                .port();
            let base_url = format!("http://127.0.0.1:{port}");

            tokio::spawn(async move {
                axum::serve(listener, router.into_make_service())
                    .await
                    .unwrap();
            });

            let client = reqwest::Client::builder()
                .default_headers({
                    let mut h = reqwest::header::HeaderMap::new();
                    h.insert(
                        reqwest::header::AUTHORIZATION,
                        reqwest::header::HeaderValue::from_static(BENCH_AUTH_HEADER),
                    );
                    h
                })
                .build()
                .unwrap();

            client
                .post(format!("{base_url}/startup/user"))
                .json(&json!({ "Name": "bench", "Password": "bench" }))
                .send()
                .await
                .unwrap();
            client
                .post(format!("{base_url}/startup/complete"))
                .send()
                .await
                .unwrap();

            let resp: serde_json::Value = client
                .post(format!("{base_url}/users/authenticatebyname"))
                .json(&json!({ "Username": "bench", "Pw": "bench" }))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let token = resp["AccessToken"]
                .as_str()
                .unwrap()
                .to_string();
            let user_id = Uuid::parse_str(
                resp["User"]["Id"]
                    .as_str()
                    .unwrap(),
            )
            .unwrap();

            seed_all(&ctx.db, user_id).await;

            (client, ctx, token, base_url)
        });

        Fixture {
            client,
            base_url,
            token,
            rt,
            _ctx: ctx,
        }
    })
}

async fn seed_all(db: &sqlx::SqlitePool, user_id: Uuid) {
    let now = chrono::Utc::now().naive_utc();
    let old_ts = now - chrono::Duration::days(60);

    seed_series(db, user_id, now, old_ts).await;
    seed_movies(db, user_id, now).await;
    seed_userviews(db).await;

    sqlx::query("ANALYZE")
        .execute(db)
        .await
        .unwrap();
}

async fn seed_userviews(db: &sqlx::SqlitePool) {
    use remux_server::sdks::remux::{
        CollectionFilter, FilterGroup, FilterMatchMode, FilterRule,
    };

    let plain = db::Media {
        title: "Movies".to_string(),
        kind: db::MediaKind::Collection,
        promoted: true,
        ..Default::default()
    };
    let watched = db::Media {
        title: "Watched".to_string(),
        kind: db::MediaKind::Collection,
        collection_kind: Some(db::CollectionKind::Smart),
        promoted: true,
        collection_smart_filter: Some(CollectionFilter {
            groups: vec![FilterGroup {
                rules: vec![FilterRule::Played { value: true }],
                match_mode: FilterMatchMode::All,
            }],
            match_mode: FilterMatchMode::All,
        }),
        ..Default::default()
    };

    // seed plain collection via upsert (no smart filter to persist)
    db::Media::upsert(db, &[plain])
        .await
        .unwrap();
    // seed watched collection via save so collection_smart_filter is stored
    let mut w = watched;
    w.save(db)
        .await
        .unwrap();
}

async fn seed_series(
    db: &sqlx::SqlitePool,
    user_id: Uuid,
    now: chrono::NaiveDateTime,
    old_ts: chrono::NaiveDateTime,
) {
    let mut items: Vec<db::Media> =
        Vec::with_capacity(TOTAL_SERIES * (2 + EPISODES as usize));
    let mut state: Vec<(Uuid, bool, chrono::NaiveDateTime)> =
        Vec::with_capacity(ACTIVE_SERIES);

    for i in 0..TOTAL_SERIES {
        let imdb = db::NonEmptyString::try_new(format!("tt{i:07}")).unwrap();

        let series_id = Uuid::from(&db::MediaIdRaw {
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(imdb.clone()),
                ..Default::default()
            },
            season: None,
            episode: None,
        });
        // released_at required: release date filter excludes items without it.
        let released_days_ago = (i as i64 * 730) / TOTAL_SERIES as i64 + 30;
        items.push(db::Media {
            id: series_id,
            title: format!("Bench Series {i}"),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(imdb.clone()),
                ..Default::default()
            },
            released_at: Some(now - chrono::Duration::days(released_days_ago)),
            ..Default::default()
        });

        let season_id = remux_server::stable_media_uuid(
            &db::MediaKind::Season,
            &format!("{}:1", series_id),
        );
        items.push(db::Media {
            id: season_id,
            title: format!("Bench Series {i} Season 1"),
            kind: db::MediaKind::Season,
            grandparent_id: Some(series_id),
            parent_id: Some(series_id),
            idx: Some(1),
            parent_idx: Some(1),
            ..Default::default()
        });

        for ep in 1..=EPISODES {
            let ep_id = remux_server::stable_media_uuid(
                &db::MediaKind::Episode,
                &format!("{}:{ep}", season_id),
            );
            items.push(db::Media {
                id: ep_id,
                title: format!("Bench Series {i} S01E{ep:02}"),
                kind: db::MediaKind::Episode,
                grandparent_id: Some(series_id),
                parent_id: Some(season_id),
                parent_idx: Some(1),
                idx: Some(ep),
                ..Default::default()
            });

            if i < ACTIVE_SERIES {
                let ts = if i < OLD_ACTIVE {
                    old_ts
                } else {
                    now - chrono::Duration::days(i as i64 % 90)
                };
                let in_progress = i >= IN_PROGRESS_START;
                if !in_progress && ep == 1 {
                    state.push((ep_id, true, ts));
                } else if in_progress && ep == 2 {
                    state.push((ep_id, false, ts));
                }
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
    for (ep_id, is_played, ts) in state {
        if is_played {
            sqlx::query(
                "INSERT OR IGNORE INTO user_media_state \
                 (user_id, media_id, media_raw, stream_id, favorite, play_count, \
                  played_at, playback_position, last_played_at, subtitle_idx, audio_idx) \
                 VALUES (?, ?, NULL, NULL, 0, 1, ?, 0, ?, NULL, NULL)",
            )
            .bind(user_id)
            .bind(ep_id)
            .bind(ts)
            .bind(ts)
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
            .bind(ts)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit()
        .await
        .unwrap();
}

async fn seed_movies(db: &sqlx::SqlitePool, user_id: Uuid, now: chrono::NaiveDateTime) {
    let mut items: Vec<db::Media> = Vec::with_capacity(TOTAL_MOVIES);

    for i in 0..TOTAL_MOVIES {
        let imdb = db::NonEmptyString::try_new(format!("mv{i:07}")).unwrap();
        let movie_id = Uuid::from(&db::MediaIdRaw {
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: Some(imdb.clone()),
                ..Default::default()
            },
            season: None,
            episode: None,
        });
        // Spread created_at evenly over the last 730 days so DateCreated sort
        // exercises a realistic distribution rather than all rows at the same timestamp.
        let days_ago = (i as i64 * 730) / TOTAL_MOVIES as i64;
        items.push(db::Media {
            id: movie_id,
            title: format!("Bench Movie {i}"),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: Some(imdb),
                ..Default::default()
            },
            created_at: now - chrono::Duration::days(days_ago),
            ..Default::default()
        });
    }

    db::Media::upsert(db, &items)
        .await
        .unwrap();

    let mut tx = db
        .begin()
        .await
        .unwrap();
    for i in 0..TOTAL_MOVIES {
        let imdb = db::NonEmptyString::try_new(format!("mv{i:07}")).unwrap();
        let movie_id = Uuid::from(&db::MediaIdRaw {
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: Some(imdb),
                ..Default::default()
            },
            season: None,
            episode: None,
        });
        let ts = now - chrono::Duration::days(i as i64 % 90);

        if i < MOVIES_IN_PROGRESS {
            sqlx::query(
                "INSERT OR IGNORE INTO user_media_state \
                 (user_id, media_id, media_raw, stream_id, favorite, play_count, \
                  played_at, playback_position, last_played_at, subtitle_idx, audio_idx) \
                 VALUES (?, ?, NULL, NULL, 0, 0, NULL, 300, ?, NULL, NULL)",
            )
            .bind(user_id)
            .bind(movie_id)
            .bind(ts)
            .execute(&mut *tx)
            .await
            .unwrap();
        } else if i < MOVIES_IN_PROGRESS + MOVIES_PLAYED {
            sqlx::query(
                "INSERT OR IGNORE INTO user_media_state \
                 (user_id, media_id, media_raw, stream_id, favorite, play_count, \
                  played_at, playback_position, last_played_at, subtitle_idx, audio_idx) \
                 VALUES (?, ?, NULL, NULL, 0, 1, ?, 0, ?, NULL, NULL)",
            )
            .bind(user_id)
            .bind(movie_id)
            .bind(ts)
            .bind(ts)
            .execute(&mut *tx)
            .await
            .unwrap();
        }
    }
    tx.commit()
        .await
        .unwrap();
}

pub fn auth_header(token: &str) -> String {
    format!("MediaBrowser Token=\"{token}\"")
}

#[derive(Clone)]
pub struct BenchQuery {
    pub name: String,
    pub url: String,
}

impl std::fmt::Display for BenchQuery {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.name)
    }
}

pub trait IntoBench {
    fn into_bench(self, path: &str) -> BenchQuery;
}

impl IntoBench for remux_server::sdks::remux::GetItemsQuery {
    fn into_bench(self, path: &str) -> BenchQuery {
        let params = serde_urlencoded::to_string(&self).unwrap_or_default();
        let url = if params.is_empty() {
            path.to_string()
        } else {
            format!("{path}?{params}")
        };
        BenchQuery { name: params, url }
    }
}

pub fn run_bench(b: &mut criterion::Bencher, url: &str) {
    let f = fixture();
    let full_url = format!("{}{}", f.base_url, url);
    let auth = auth_header(&f.token);
    b.iter(|| {
        f.rt.block_on(async {
            f.client
                .get(&full_url)
                .header(reqwest::header::AUTHORIZATION, &auth)
                .send()
                .await
                .unwrap()
        })
    });
}
