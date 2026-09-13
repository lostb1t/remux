use anyhow::Result;
use async_trait::async_trait;
use chrono::Datelike as _;
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
            offset += PAGE_SIZE;
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
        refresh_latest_flags(&ctx.db).await?;
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
    let today = chrono::Utc::now().date_naive();
    let week_start = today
        - chrono::Duration::days(
            today
                .weekday()
                .num_days_from_monday() as i64,
        );
    let month = today
        .format("%Y-%m")
        .to_string();
    let year = today
        .format("%Y")
        .to_string();
    let mut media = Vec::with_capacity(synced.len());
    let mut rows = Vec::with_capacity(synced.len() * 6);
    for (mut item, metrics) in synced {
        if let Some(ratings) = metrics.ratings {
            let tomatoes = ratings
                .tomatoes
                .filter(|score| score.is_finite() && (0.0..=100.0).contains(score));
            item.rating_audience = Some(ratings.score_average);
            item.rating_critic = tomatoes;
            item.external_ratings
                .get_or_insert_default()
                .remuxdb = Some(db::RemuxDbRatings {
                score: ratings.score,
                score_average: ratings.score_average,
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
        push_metric(
            &mut rows,
            item.id,
            "daily",
            today.to_string(),
            metrics
                .popularity
                .daily,
        );
        push_metric(
            &mut rows,
            item.id,
            "weekly",
            week_start.to_string(),
            metrics
                .popularity
                .weekly,
        );
        push_metric(
            &mut rows,
            item.id,
            "monthly",
            month.clone(),
            metrics
                .popularity
                .monthly,
        );
        push_metric(
            &mut rows,
            item.id,
            "yearly",
            year.clone(),
            metrics
                .popularity
                .yearly,
        );
        push_metric(
            &mut rows,
            item.id,
            "trend_week",
            today.to_string(),
            metrics
                .trending
                .weekly,
        );
        push_metric(
            &mut rows,
            item.id,
            "trend_month",
            today.to_string(),
            metrics
                .trending
                .monthly,
        );
        media.push(item);
    }
    db::Media::upsert(pool, &media).await?;
    for chunk in rows.chunks(400) {
        let mut query = sqlx::QueryBuilder::new(
            "INSERT INTO popularity_agg (media_id, period, period_key, avg, min, max, sample_count, latest) ",
        );
        query.push_values(chunk, |mut b, (media_id, period, period_key, value)| {
            b.push_bind(media_id)
                .push_bind(period)
                .push_bind(period_key)
                .push_bind(value)
                .push_bind(value)
                .push_bind(value)
                .push_bind(1_i64)
                .push_bind(1_i64);
        });
        query.push(
            " ON CONFLICT(media_id, period, period_key) DO UPDATE SET \
             avg = excluded.avg, min = excluded.min, max = excluded.max, \
             sample_count = excluded.sample_count, latest = excluded.latest",
        );
        query
            .build()
            .execute(pool)
            .await?;
    }
    Ok(())
}

fn push_metric(
    rows: &mut Vec<(uuid::Uuid, &'static str, String, f64)>,
    media_id: uuid::Uuid,
    period: &'static str,
    period_key: String,
    value: Option<f64>,
) {
    if let Some(value) = value.filter(|value| value.is_finite()) {
        rows.push((media_id, period, period_key, value));
    }
}

async fn refresh_latest_flags(pool: &sqlx::SqlitePool) -> Result<()> {
    for period in [
        "daily",
        "weekly",
        "monthly",
        "yearly",
        "trend_week",
        "trend_month",
    ] {
        sqlx::query("UPDATE popularity_agg SET latest = 0 WHERE period = ?")
            .bind(period)
            .execute(pool)
            .await?;
        sqlx::query(
            "UPDATE popularity_agg SET latest = 1 WHERE period = ? \
             AND (media_id, period_key) IN (SELECT media_id, MAX(period_key) \
             FROM popularity_agg WHERE period = ? GROUP BY media_id)",
        )
        .bind(period)
        .bind(period)
        .execute(pool)
        .await?;
    }
    Ok(())
}
