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
            let page = db::Media::get_by_filter(
                &ctx.db,
                &db::MediaFilter {
                    kind: Some(vec![db::MediaKind::Movie, db::MediaKind::Series]),
                    limit: Some(PAGE_SIZE),
                    offset: Some(offset),
                    total_count: false,
                    ..Default::default()
                },
            )
            .await?
            .records;
            if page.is_empty() {
                break;
            }
            offset += page.len() as u32;
            let page: Vec<_> = page
                .into_iter()
                .filter(|media| {
                    media
                        .external_ids
                        .imdb
                        .is_some()
                })
                .collect();
            if page.is_empty() {
                continue;
            }
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
                .tomatoes
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
