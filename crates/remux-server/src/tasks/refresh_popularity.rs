use anyhow::Result;
use async_trait::async_trait;
use futures::{StreamExt as _, stream};
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, db};

pub struct RefreshPopularityTask;

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

            let synced: Vec<_> = stream::iter(page)
                .map(|media| {
                    let base_url = base_url.to_string();
                    let client_id = client_id.clone();
                    async move {
                        let imdb_id = media
                            .external_ids
                            .imdb
                            .clone()?;
                        remux_sdks::remuxdb::fetch_media_metrics(
                            &base_url, &client_id, &imdb_id,
                        )
                        .await
                        .map(|metrics| (media, metrics))
                    }
                })
                .buffer_unordered(concurrency)
                .filter_map(|synced| async move { synced })
                .collect()
                .await;
            persist_metrics(&ctx.db, synced).await?;
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
        media.push(item);
    }
    db::Media::upsert(pool, &media).await?;
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
        let media = db::Media {
            title: "The Godfather".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0068646".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let media_id = media.id;
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
        let media = db::Media {
            title: "Obscure Movie".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt9999999".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let media_id = media.id;
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
}
