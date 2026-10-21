use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::{StreamExt as _, stream};
use remux_sdks::remuxdb::{MediaMetrics, MetricsLookup};
use std::{future::Future, sync::Arc, time::Duration};
use tracing::{info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, db};

pub struct RefreshPopularityTask;

/// Pauses before each retry round for titles RemuxDB could not answer (rate
/// limit, 5xx, Cloudflare 52x). Outages seen in production clear within a few
/// minutes, so the waits grow instead of hammering a struggling backend.
const RETRY_DELAYS: [Duration; 4] = [
    Duration::from_secs(15),
    Duration::from_secs(30),
    Duration::from_secs(60),
    Duration::from_secs(120),
];

/// A page is only worth waiting for when more than this share of it came back
/// unavailable. A few titles that fail on their own (a bad id, one poisoned
/// record) must not stall the whole run while the backend is healthy.
const BACKOFF_FAILURE_SHARE: f64 = 0.10;

/// Total time one run may spend sleeping between retry rounds, across all
/// pages. Once spent, unavailable titles are only counted and logged.
const BACKOFF_WAIT_BUDGET: Duration = Duration::from_secs(10 * 60);

/// Looks up every item, then re-queues the ones that came back
/// [`MetricsLookup::Unavailable`] after each pause in `delays`, with a
/// quarter of the original concurrency every round. Retries only happen while
/// more than [`BACKOFF_FAILURE_SHARE`] of `items` is still unavailable and the
/// next pause fits in `wait_budget`, which is shared across calls. Returns
/// what was found, the items RemuxDB has no entry for, and how many items
/// were still unavailable at the end.
async fn lookup_with_backoff<T, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    delays: &[Duration],
    wait_budget: &mut Duration,
    lookup: F,
) -> (Vec<(T, MediaMetrics)>, Vec<T>, usize)
where
    F: Fn(&T) -> Fut,
    Fut: Future<Output = MetricsLookup>,
{
    let page_len = items.len();
    let first_round = concurrency.max(1);
    let retry_round = first_round
        .div_ceil(4)
        .max(1);
    let mut round_concurrency = first_round;
    let mut found = Vec::new();
    let mut missing = Vec::new();
    let mut rejected = 0_usize;
    let mut pending = items;
    let mut delays = delays.iter();
    loop {
        let results: Vec<_> = stream::iter(pending)
            .map(|item| {
                let lookup = &lookup;
                async move {
                    let result = lookup(&item).await;
                    (item, result)
                }
            })
            .buffer_unordered(round_concurrency)
            .collect()
            .await;
        pending = Vec::new();
        for (item, result) in results {
            match result {
                MetricsLookup::Found(metrics) => found.push((item, metrics)),
                MetricsLookup::NotFound => missing.push(item),
                // Neither stamped nor retried: a blocked or malformed answer
                // says nothing about the title, and a systemic failure must
                // not push every title into the weekly tier.
                MetricsLookup::Rejected => rejected += 1,
                MetricsLookup::Unavailable => pending.push(item),
            }
        }
        let widespread = pending.len() as f64 > page_len as f64 * BACKOFF_FAILURE_SHARE;
        let delay = delays
            .next()
            .filter(|delay| widespread && **delay <= *wait_budget);
        let Some(delay) = delay else {
            if rejected > 0 {
                warn!(
                    rejected,
                    "RemuxDB rejected metrics requests; not marking those titles as synced"
                );
            }
            return (found, missing, pending.len());
        };
        warn!(
            unavailable = pending.len(),
            retry_in_secs = delay.as_secs(),
            "RemuxDB metrics unavailable; backing off before retrying"
        );
        *wait_budget -= *delay;
        tokio::time::sleep(*delay).await;
        round_concurrency = retry_round;
    }
}

/// More than half of a page still unavailable after the retries.
fn is_outage(gave_up: usize, page_len: usize) -> bool {
    gave_up * 2 > page_len
}

#[async_trait]
impl Task for RefreshPopularityTask {
    fn key(&self) -> &str {
        "RefreshPopularity"
    }
    fn name(&self) -> &str {
        "Sync RemuxDB Metrics"
    }
    fn description(&self) -> &str {
        "Syncs popularity, trending, and ratings from RemuxDB for movies and series in your library."
    }
    fn short_description(&self) -> &str {
        "Syncs popularity, trending, and ratings from RemuxDB"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Library
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let Some(base_url) = ctx
            .config
            .remuxdb_url
            .as_deref()
        else {
            return Ok(());
        };
        let ids =
            db::Media::popularity_sync_ids(&ctx.db, Utc::now().naive_utc()).await?;
        let total = ids.len();
        info!(titles = total, "RemuxDB metrics sync starting");
        let concurrency = db::Settings::get_config_or_default(&ctx.db)
            .await
            .meta_concurrency
            .max(1) as usize;
        let client_id = crate::common::server_id().to_string();

        const PAGE_SIZE: usize = 250;
        let mut completed = 0_usize;
        let mut wait_budget = BACKOFF_WAIT_BUDGET;
        for chunk in ids.chunks(PAGE_SIZE) {
            let page = db::Media::list_for_popularity_sync(&ctx.db, chunk).await?;
            completed += chunk.len();

            let page_len = page.len();
            let (synced, missing, gave_up) = lookup_with_backoff(
                page,
                concurrency,
                &RETRY_DELAYS,
                &mut wait_budget,
                |media| {
                    let base_url = base_url.to_string();
                    let client_id = client_id.clone();
                    let imdb_id = media
                        .external_ids
                        .imdb
                        .clone();
                    async move {
                        match imdb_id {
                            Some(imdb_id) => {
                                remux_sdks::remuxdb::lookup_media_metrics(
                                    &base_url, &client_id, &imdb_id,
                                )
                                .await
                            }
                            None => MetricsLookup::NotFound,
                        }
                    }
                },
            )
            .await;
            if gave_up > 0 {
                warn!(
                    gave_up,
                    "RemuxDB metrics still unavailable after retries; they will sync on the next run"
                );
            }
            // A page that mostly failed means the backend is down, not flaky;
            // stop instead of waiting out the same delays for every page.
            let outage = is_outage(gave_up, page_len);
            persist_metrics(&ctx.db, synced).await?;
            mark_synced(
                &ctx.db,
                &missing
                    .iter()
                    .map(|media| media.id)
                    .collect::<Vec<_>>(),
            )
            .await?;
            if outage {
                anyhow::bail!("RemuxDB is unavailable; metrics sync stopped early");
            }
            progress.set((completed as f64 / total as f64 * 100.0).min(99.0));
        }
        info!("RemuxDB metrics sync complete");
        progress.set(100.0);
        Ok(())
    }
}

/// Records that we asked RemuxDB about these titles and it had nothing, so the
/// tiered selection does not treat them as never synced and ask again every
/// day. Existing metric values are left alone.
async fn mark_synced(pool: &sqlx::SqlitePool, ids: &[uuid::Uuid]) -> Result<()> {
    for chunk in ids.chunks(100) {
        let mut query =
            sqlx::QueryBuilder::new("INSERT INTO media_metrics (media_id, synced_at) ");
        query.push_values(chunk, |mut b, id| {
            b.push_bind(*id)
                .push("CURRENT_TIMESTAMP");
        });
        query.push(
            " ON CONFLICT(media_id) DO UPDATE SET synced_at = excluded.synced_at",
        );
        query
            .build()
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn persist_metrics(
    pool: &sqlx::SqlitePool,
    synced: Vec<(db::Media, remux_sdks::remuxdb::MediaMetrics)>,
) -> Result<()> {
    if synced.is_empty() {
        return Ok(());
    }
    let mut media = Vec::with_capacity(synced.len());
    let mut rows = Vec::with_capacity(synced.len());
    for (mut item, metrics) in synced {
        if let Some(ratings) = metrics.ratings {
            let score = ratings
                .score
                .filter(|score| score.is_finite());
            let score_average = ratings
                .score_average
                .filter(|score| score.is_finite());
            let tomatoes = ratings
                .sources
                .iter()
                .find(|source| source.source == "tomatoes")
                .map(|source| source.value)
                .filter(|score| score.is_finite() && (0.0..=100.0).contains(score));
            item.rating_audience = score_average;
            item.rating_critic = tomatoes;
            item.external_ratings
                .get_or_insert_default()
                .remuxdb = Some(db::RemuxDbRatings {
                score,
                score_average,
                tomatoes,
                sources: ratings
                    .sources
                    .into_iter()
                    .map(|source| db::RemuxDbRatingSource {
                        source: source.source,
                        value: source.value,
                        votes: source.votes,
                    })
                    .collect(),
                updated_at: ratings.updated_at,
            });
        }
        rows.push((
            item.id,
            metrics
                .popularity
                .all_time
                .filter(|value| value.is_finite()),
            metrics
                .popularity
                .daily
                .filter(|value| value.is_finite()),
            metrics
                .popularity
                .weekly
                .filter(|value| value.is_finite()),
            metrics
                .popularity
                .monthly
                .filter(|value| value.is_finite()),
            metrics
                .popularity
                .yearly
                .filter(|value| value.is_finite()),
            metrics
                .trending
                .weekly
                .filter(|value| value.is_finite()),
            metrics
                .trending
                .monthly
                .filter(|value| value.is_finite()),
        ));
        // Only items that got ratings have anything to write to `media`.
        if item
            .external_ratings
            .is_some()
        {
            media.push(item);
        }
    }
    db::Media::update_ratings(pool, &media).await?;
    for chunk in rows.chunks(100) {
        let mut query = sqlx::QueryBuilder::new(
            "INSERT INTO media_metrics (\
             media_id, popularity_all_time, popularity_daily, popularity_weekly, popularity_monthly, \
             popularity_yearly, trending_weekly, trending_monthly, synced_at\
             ) ",
        );
        query.push_values(chunk, |mut b, row| {
            b.push_bind(row.0)
                .push_bind(row.1)
                .push_bind(row.2)
                .push_bind(row.3)
                .push_bind(row.4)
                .push_bind(row.5)
                .push_bind(row.6)
                .push_bind(row.7)
                .push("CURRENT_TIMESTAMP");
        });
        query.push(
            " ON CONFLICT(media_id) DO UPDATE SET \
             popularity_all_time = excluded.popularity_all_time, \
             popularity_daily = excluded.popularity_daily, \
             popularity_weekly = excluded.popularity_weekly, \
             popularity_monthly = excluded.popularity_monthly, \
             popularity_yearly = excluded.popularity_yearly, \
             trending_weekly = excluded.trending_weekly, \
             trending_monthly = excluded.trending_monthly, \
             synced_at = excluded.synced_at",
        );
        query
            .build()
            .execute(pool)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use remux_sdks::remuxdb::{
        MediaMetrics, MediaRatings, MetricPeriods, RatingSource,
    };

    fn empty_metrics() -> MediaMetrics {
        MediaMetrics {
            popularity: MetricPeriods::default(),
            trending: MetricPeriods::default(),
            ratings: None,
        }
    }

    /// A backend that answers 503 for each item's first `failures` calls.
    fn flaky(
        failures: usize,
    ) -> (
        Arc<std::sync::Mutex<std::collections::HashMap<u32, usize>>>,
        impl Fn(&u32) -> std::future::Ready<MetricsLookup>,
    ) {
        let calls = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let seen = calls.clone();
        let lookup = move |item: &u32| {
            let mut calls = seen
                .lock()
                .unwrap();
            let count = calls
                .entry(*item)
                .or_default();
            *count += 1;
            std::future::ready(if *count > failures {
                MetricsLookup::Found(empty_metrics())
            } else {
                MetricsLookup::Unavailable
            })
        };
        (calls, lookup)
    }

    #[tokio::test]
    async fn unavailable_items_are_retried_until_they_succeed() {
        let (calls, lookup) = flaky(2);
        let delays = [Duration::ZERO; 3];

        let (found, _missing, gave_up) = lookup_with_backoff(
            (0..20).collect::<Vec<u32>>(),
            4,
            &delays,
            &mut Duration::MAX,
            lookup,
        )
        .await;

        assert_eq!(found.len(), 20);
        assert_eq!(gave_up, 0);
        assert!(
            calls
                .lock()
                .unwrap()
                .values()
                .all(|count| *count == 3)
        );
    }

    #[tokio::test]
    async fn items_still_unavailable_after_the_last_delay_are_counted() {
        let (calls, lookup) = flaky(usize::MAX);
        let delays = [Duration::ZERO; 2];

        let (found, _missing, gave_up) = lookup_with_backoff(
            vec![1_u32, 2, 3],
            2,
            &delays,
            &mut Duration::MAX,
            lookup,
        )
        .await;

        assert!(found.is_empty());
        assert_eq!(gave_up, 3);
        // first pass plus one per delay, never more
        assert!(
            calls
                .lock()
                .unwrap()
                .values()
                .all(|count| *count == 3)
        );
    }

    #[tokio::test]
    async fn not_found_titles_are_not_retried() {
        let calls = std::sync::atomic::AtomicUsize::new(0);

        let (found, _missing, gave_up) = lookup_with_backoff(
            vec![1_u32],
            1,
            &[Duration::ZERO; 3],
            &mut Duration::MAX,
            |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(MetricsLookup::NotFound)
            },
        )
        .await;

        assert!(found.is_empty());
        assert_eq!(gave_up, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
    }

    /// Items in `bad` always answer 503, everything else succeeds at once.
    fn some_always_down(
        bad: std::ops::Range<u32>,
    ) -> (
        Arc<std::sync::atomic::AtomicUsize>,
        impl Fn(&u32) -> std::future::Ready<MetricsLookup>,
    ) {
        let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let seen = calls.clone();
        let lookup = move |item: &u32| {
            seen.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            std::future::ready(if bad.contains(item) {
                MetricsLookup::Unavailable
            } else {
                MetricsLookup::Found(empty_metrics())
            })
        };
        (calls, lookup)
    }

    #[tokio::test]
    async fn a_few_failures_on_a_page_are_not_retried() {
        // 10 of 100 is exactly the threshold, not above it
        let (calls, lookup) = some_always_down(0..10);
        let mut budget = Duration::from_secs(60);

        let (found, _missing, gave_up) = lookup_with_backoff(
            (0..100).collect::<Vec<u32>>(),
            4,
            &[Duration::from_secs(5); 3],
            &mut budget,
            lookup,
        )
        .await;

        assert_eq!(found.len(), 90);
        assert_eq!(gave_up, 10);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 100);
        // nothing was slept, so nothing was spent
        assert_eq!(budget, Duration::from_secs(60));
    }

    #[tokio::test]
    async fn widespread_failures_are_retried() {
        let (calls, lookup) = some_always_down(0..11);

        let (_, _, gave_up) = lookup_with_backoff(
            (0..100).collect::<Vec<u32>>(),
            4,
            &[Duration::ZERO; 2],
            &mut Duration::MAX,
            lookup,
        )
        .await;

        assert_eq!(gave_up, 11);
        assert_eq!(
            calls.load(std::sync::atomic::Ordering::SeqCst),
            100 + 2 * 11
        );
    }

    #[tokio::test]
    async fn the_wait_budget_is_shared_and_stops_further_backoff() {
        let delays = [Duration::from_millis(10); 4];
        let mut budget = Duration::from_millis(25);
        let (calls, lookup) = some_always_down(0..u32::MAX);

        // two sleeps fit in the budget, the third would not
        let (_, _, gave_up) =
            lookup_with_backoff(vec![1_u32, 2], 2, &delays, &mut budget, &lookup).await;
        assert_eq!(gave_up, 2);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2 * 3);
        assert_eq!(budget, Duration::from_millis(5));

        // the next page gets one pass and no waiting at all
        let (_, _, gave_up) =
            lookup_with_backoff(vec![3_u32, 4], 2, &delays, &mut budget, &lookup).await;
        assert_eq!(gave_up, 2);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2 * 3 + 2);
        assert_eq!(budget, Duration::from_millis(5));
    }

    #[tokio::test]
    async fn retry_rounds_use_a_quarter_of_the_original_concurrency() {
        use std::sync::atomic::{AtomicUsize, Ordering::SeqCst};
        let active = AtomicUsize::new(0);
        // highest number of lookups in flight, per round (40 items each)
        let peaks = std::sync::Mutex::new(vec![0_usize; 4]);
        let started = AtomicUsize::new(0);

        let (_, _, gave_up) = lookup_with_backoff(
            (0..40).collect::<Vec<u32>>(),
            16,
            &[Duration::ZERO; 3],
            &mut Duration::MAX,
            |_| {
                let round = started.fetch_add(1, SeqCst) / 40;
                let now = active.fetch_add(1, SeqCst) + 1;
                {
                    let mut peaks = peaks
                        .lock()
                        .unwrap();
                    peaks[round] = peaks[round].max(now);
                }
                let active = &active;
                async move {
                    tokio::task::yield_now().await;
                    active.fetch_sub(1, SeqCst);
                    MetricsLookup::Unavailable
                }
            },
        )
        .await;

        assert_eq!(gave_up, 40);
        // 16, then ceil(16 / 4) = 4 every round, not 4, 1, 1
        assert_eq!(
            *peaks
                .lock()
                .unwrap(),
            vec![16, 4, 4, 4]
        );
    }

    #[test]
    fn a_page_that_mostly_failed_stops_the_run() {
        assert!(is_outage(126, 250));
        assert!(is_outage(250, 250));
        assert!(!is_outage(125, 250));
        assert!(!is_outage(0, 250));
    }

    #[tokio::test]
    async fn a_mostly_failing_page_reports_an_outage_after_the_retries() {
        let (_, lookup) = some_always_down(0..60);
        let page_len = 100;

        let (_, _, gave_up) = lookup_with_backoff(
            (0..page_len as u32).collect::<Vec<u32>>(),
            4,
            &[Duration::ZERO; 2],
            &mut Duration::MAX,
            lookup,
        )
        .await;

        assert!(is_outage(gave_up, page_len));
    }

    fn metrics_with_sources(sources: Vec<(&str, f64)>) -> MediaMetrics {
        MediaMetrics {
            popularity: MetricPeriods::default(),
            trending: MetricPeriods::default(),
            ratings: Some(MediaRatings {
                score: Some(80.0),
                score_average: Some(8.0),
                sources: sources
                    .into_iter()
                    .map(|(source, value)| RatingSource {
                        source: source.to_string(),
                        value,
                        votes: None,
                    })
                    .collect(),
                updated_at: None,
            }),
        }
    }

    #[tokio::test]
    async fn critic_rating_comes_from_the_tomatoes_source_entry() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let mut media = db::Media {
            title: "The Godfather".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0068646".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let media_id = media.id;
        media
            .save(&ctx.db)
            .await
            .unwrap();
        let metrics = metrics_with_sources(vec![
            ("imdb", 9.2),
            ("tomatoes", 97.0),
            ("tomatoesaudience", 98.0),
        ]);

        persist_metrics(&ctx.db, vec![(media, metrics)])
            .await
            .unwrap();

        let stored = db::Media::get_by_id(&ctx.db, &media_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.rating_critic, Some(97.0));
    }

    /// No `tomatoes` entry anywhere leaves the critic rating unset rather than
    /// guessing from an unrelated source.
    #[tokio::test]
    async fn critic_rating_stays_unset_without_a_tomatoes_source() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let mut media = db::Media {
            title: "Obscure Movie".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt9999999".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let media_id = media.id;
        media
            .save(&ctx.db)
            .await
            .unwrap();
        let metrics = metrics_with_sources(vec![("imdb", 5.0)]);

        persist_metrics(&ctx.db, vec![(media, metrics)])
            .await
            .unwrap();

        let stored = db::Media::get_by_id(&ctx.db, &media_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.rating_critic, None);
    }

    /// The sync loads a minimal projection and upserts it back, so it must not
    /// null out columns it never loaded.
    #[tokio::test]
    async fn sync_keeps_certification_and_certification_age() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let mut media = db::Media {
            title: "Rated Movie".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0111161".to_string()).ok(),
                ..Default::default()
            },
            certification: Some("PG-13".to_string()),
            certification_age: Some(13),
            ..Default::default()
        };
        let media_id = media.id;
        media
            .save(&ctx.db)
            .await
            .unwrap();

        let page = db::Media::list_for_popularity_sync(&ctx.db, &[media_id])
            .await
            .unwrap();
        let synced = page
            .into_iter()
            .map(|m| (m, metrics_with_sources(vec![("tomatoes", 90.0)])))
            .collect();
        persist_metrics(&ctx.db, synced)
            .await
            .unwrap();

        let stored = db::Media::get_by_id(&ctx.db, &media_id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            stored
                .certification
                .as_deref(),
            Some("PG-13")
        );
        assert_eq!(stored.certification_age, Some(13));
    }

    fn now() -> chrono::NaiveDateTime {
        chrono::NaiveDate::from_ymd_opt(2026, 10, 20)
            .unwrap()
            .and_hms_opt(12, 0, 0)
            .unwrap()
    }

    /// Saves a title and, when `synced_days_ago` is set, a metrics row stamped
    /// that many days before [`now`].
    async fn seed(
        ctx: &AppContext,
        imdb: &str,
        kind: db::MediaKind,
        released_days_ago: Option<i64>,
        status: Option<db::MediaStatus>,
        synced_days_ago: Option<i64>,
    ) -> uuid::Uuid {
        let mut media = db::Media {
            title: imdb.to_string(),
            kind,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new(imdb.to_string()).ok(),
                ..Default::default()
            },
            released_at: released_days_ago
                .map(|days| now() - chrono::Duration::days(days)),
            status,
            ..Default::default()
        };
        let id = media.id;
        media
            .save(&ctx.db)
            .await
            .unwrap();
        if let Some(days) = synced_days_ago {
            sqlx::query(
                "INSERT INTO media_metrics (media_id, synced_at) VALUES (?, ?)",
            )
            .bind(id)
            .bind(
                (now() - chrono::Duration::days(days))
                    .format("%Y-%m-%d %H:%M:%S")
                    .to_string(),
            )
            .execute(&ctx.db)
            .await
            .unwrap();
        }
        id
    }

    const OLD: Option<i64> = Some(20 * 365);

    #[tokio::test]
    async fn daily_tier_covers_recent_upcoming_running_and_never_synced() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let movie = db::MediaKind::Movie;
        let series = db::MediaKind::Series;
        let recent =
            seed(ctx, "tt1000001", movie.clone(), Some(100), None, Some(1)).await;
        let upcoming =
            seed(ctx, "tt1000002", movie.clone(), Some(-30), None, Some(1)).await;
        let running = seed(
            ctx,
            "tt1000003",
            series.clone(),
            OLD,
            Some(db::MediaStatus::Continuing),
            Some(1),
        )
        .await;
        let never = seed(ctx, "tt1000004", movie.clone(), OLD, None, None).await;
        let ended = seed(
            ctx,
            "tt1000005",
            series,
            OLD,
            Some(db::MediaStatus::Ended),
            Some(2),
        )
        .await;
        let old_fresh = seed(ctx, "tt1000006", movie, OLD, None, Some(2)).await;

        let ids = db::Media::popularity_sync_ids(&ctx.db, now())
            .await
            .unwrap();

        for id in [recent, upcoming, running, never] {
            assert!(ids.contains(&id));
        }
        assert!(!ids.contains(&ended), "ended series synced 2 days ago");
        assert!(!ids.contains(&old_fresh), "old title synced 2 days ago");
    }

    #[tokio::test]
    async fn old_titles_come_back_once_their_metrics_are_a_week_old() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let movie = db::MediaKind::Movie;
        let six = seed(ctx, "tt2000001", movie.clone(), OLD, None, Some(6)).await;
        let eight = seed(ctx, "tt2000002", movie, OLD, None, Some(8)).await;

        let ids = db::Media::popularity_sync_ids(&ctx.db, now())
            .await
            .unwrap();

        assert_eq!(ids, vec![eight]);
        assert!(!ids.contains(&six));
    }

    /// A first rollout leaves every row with a similar age; the weekly tier
    /// takes a seventh of them per run, oldest first.
    #[tokio::test]
    async fn weekly_tier_is_capped_and_takes_the_oldest_first() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let mut by_age = Vec::new();
        for n in 0..15 {
            let id = seed(
                ctx,
                &format!("tt30000{n:02}"),
                db::MediaKind::Movie,
                OLD,
                None,
                Some(8 + n),
            )
            .await;
            by_age.push(id);
        }

        let ids = db::Media::popularity_sync_ids(&ctx.db, now())
            .await
            .unwrap();

        // ceil(15 / 7) = 3, the three that have waited longest
        assert_eq!(ids.len(), 3);
        for id in &by_age[12..] {
            assert!(ids.contains(id));
        }
    }

    #[tokio::test]
    async fn titles_remuxdb_does_not_know_are_not_asked_again_daily() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let id = seed(ctx, "tt4000001", db::MediaKind::Movie, OLD, None, None).await;
        assert!(
            db::Media::popularity_sync_ids(&ctx.db, now())
                .await
                .unwrap()
                .contains(&id)
        );

        mark_synced(&ctx.db, &[id])
            .await
            .unwrap();

        assert!(
            db::Media::popularity_sync_ids(&ctx.db, chrono::Utc::now().naive_utc())
                .await
                .unwrap()
                .is_empty()
        );
    }

    /// A rejected answer (a Cloudflare 403, say) is not evidence that RemuxDB
    /// lacks the title: only the not-found ids come back to be stamped, and
    /// neither kind is retried.
    #[tokio::test]
    async fn only_not_found_titles_are_stamped() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let unknown =
            seed(ctx, "tt5000001", db::MediaKind::Movie, OLD, None, None).await;
        let blocked =
            seed(ctx, "tt5000002", db::MediaKind::Movie, OLD, None, None).await;
        let page = db::Media::list_for_popularity_sync(&ctx.db, &[unknown, blocked])
            .await
            .unwrap();
        let calls = std::sync::atomic::AtomicUsize::new(0);

        let (found, not_found, gave_up) = lookup_with_backoff(
            page,
            2,
            &[Duration::ZERO; 3],
            &mut Duration::MAX,
            |media| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(if media.id == unknown {
                    MetricsLookup::NotFound
                } else {
                    MetricsLookup::Rejected
                })
            },
        )
        .await;

        assert!(found.is_empty());
        assert_eq!(gave_up, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 2);
        let ids: Vec<_> = not_found
            .iter()
            .map(|media| media.id)
            .collect();
        assert_eq!(ids, vec![unknown]);

        mark_synced(&ctx.db, &ids)
            .await
            .unwrap();

        let due =
            db::Media::popularity_sync_ids(&ctx.db, chrono::Utc::now().naive_utc())
                .await
                .unwrap();
        assert_eq!(due, vec![blocked]);
    }
}
