use anyhow::Result;
use async_trait::async_trait;
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

/// Looks up every item, then re-queues the ones that came back
/// [`MetricsLookup::Unavailable`] after each pause in `delays`, with a
/// quarter of the concurrency. Returns what was found plus how many items
/// were still unavailable when the delays ran out.
async fn lookup_with_backoff<T, F, Fut>(
    items: Vec<T>,
    concurrency: usize,
    delays: &[Duration],
    lookup: F,
) -> (Vec<(T, MediaMetrics)>, usize)
where
    F: Fn(&T) -> Fut,
    Fut: Future<Output = MetricsLookup>,
{
    let mut found = Vec::new();
    let mut pending = items;
    let mut concurrency = concurrency.max(1);
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
            .buffer_unordered(concurrency)
            .collect()
            .await;
        pending = Vec::new();
        for (item, result) in results {
            match result {
                MetricsLookup::Found(metrics) => found.push((item, metrics)),
                MetricsLookup::Missing => {}
                MetricsLookup::Unavailable => pending.push(item),
            }
        }
        let Some(delay) = delays
            .next()
            .filter(|_| !pending.is_empty())
        else {
            return (found, pending.len());
        };
        warn!(
            unavailable = pending.len(),
            retry_in_secs = delay.as_secs(),
            "RemuxDB metrics unavailable; backing off before retrying"
        );
        tokio::time::sleep(*delay).await;
        concurrency = concurrency.div_ceil(4);
    }
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
        let total: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.imdb') IS NOT NULL",
        )
        .fetch_one(&ctx.db)
        .await?;
        let concurrency = db::Settings::get_config_or_default(&ctx.db)
            .await
            .meta_concurrency
            .max(1) as usize;
        let client_id = crate::common::server_id().to_string();

        const PAGE_SIZE: u32 = 250;
        let mut offset = 0;
        let mut completed = 0_i64;
        loop {
            let page =
                db::Media::list_for_popularity_sync(&ctx.db, PAGE_SIZE, offset).await?;
            if page.is_empty() {
                break;
            }
            offset += page.len() as u32;
            completed += page.len() as i64;

            let page_len = page.len();
            let (synced, gave_up) =
                lookup_with_backoff(page, concurrency, &RETRY_DELAYS, |media| {
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
                            None => MetricsLookup::Missing,
                        }
                    }
                })
                .await;
            if gave_up > 0 {
                warn!(
                    gave_up,
                    "RemuxDB metrics still unavailable after retries; they will sync on the next run"
                );
            }
            // A page that mostly failed means the backend is down, not flaky;
            // stop instead of waiting out the same delays for every page.
            let outage = gave_up * 2 > page_len;
            persist_metrics(&ctx.db, synced).await?;
            if outage {
                anyhow::bail!("RemuxDB is unavailable; metrics sync stopped early");
            }
            if total > 0 {
                progress.set((completed as f64 / total as f64 * 100.0).min(99.0));
            }
        }
        info!("RemuxDB metrics sync complete");
        progress.set(100.0);
        Ok(())
    }
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

        let (found, gave_up) =
            lookup_with_backoff((0..20).collect::<Vec<u32>>(), 4, &delays, lookup)
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

        let (found, gave_up) =
            lookup_with_backoff(vec![1_u32, 2, 3], 2, &delays, lookup).await;

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
    async fn missing_titles_are_not_retried() {
        let calls = std::sync::atomic::AtomicUsize::new(0);

        let (found, gave_up) =
            lookup_with_backoff(vec![1_u32], 1, &[Duration::ZERO; 3], |_| {
                calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                std::future::ready(MetricsLookup::Missing)
            })
            .await;

        assert!(found.is_empty());
        assert_eq!(gave_up, 0);
        assert_eq!(calls.load(std::sync::atomic::Ordering::SeqCst), 1);
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

        let page = db::Media::list_for_popularity_sync(&ctx.db, 10, 0)
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
}
