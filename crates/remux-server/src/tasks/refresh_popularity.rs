use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct RefreshPopularityTask;

#[async_trait]
impl Task for RefreshPopularityTask {
    fn key(&self) -> &str {
        "RefreshPopularity"
    }

    fn name(&self) -> &str {
        "Refresh Popularity Data"
    }

    fn description(&self) -> &str {
        "Fetches the latest popularity score for every item in your library and updates the historical trend data used for sorting by popularity. Run this task daily for accurate trending results."
    }

    fn short_description(&self) -> &str {
        "Updates popularity scores and trend history"
    }

    fn category(&self) -> &str {
        "Metrics"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        // --- Phase 1: fetch fresh scores from all metrics addons (0–60%) ---
        ctx.addons
            .snapshot_all_metrics(&ctx, progress.scaled(0.0, 60.0))
            .await?;
        progress.set(60.0);

        // --- Phase 2: roll up into trend buckets (60–100%) ---
        let db = &ctx.db;

        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT source, external_id, 'daily', date, \
                    AVG(value), MIN(value), MAX(value), COUNT(*) \
             FROM popularity_raw \
             GROUP BY source, external_id, date",
        )
        .execute(db)
        .await?;
        sqlx::query("DELETE FROM popularity_raw WHERE date < date('now', '-2 days')")
            .execute(db)
            .await?;
        progress.set(68.0);

        // --- trend_week: today / 7 days ago ratio ---
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT n.source, n.external_id, 'trend_week', date('now'), \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    1 \
             FROM popularity_agg n \
             LEFT JOIN popularity_agg o \
                 ON o.external_id = n.external_id AND o.source = n.source \
                 AND o.period = 'daily' AND o.period_key = date('now', '-7 days') \
             WHERE n.period = 'daily' AND n.period_key = date('now')",
        )
        .execute(db)
        .await?;

        // --- trend_month: today / 30 days ago ratio ---
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT n.source, n.external_id, 'trend_month', date('now'), \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    CASE WHEN o.avg > 0 THEN n.avg / o.avg ELSE n.avg END, \
                    1 \
             FROM popularity_agg n \
             LEFT JOIN popularity_agg o \
                 ON o.external_id = n.external_id AND o.source = n.source \
                 AND o.period = 'daily' AND o.period_key = date('now', '-30 days') \
             WHERE n.period = 'daily' AND n.period_key = date('now')",
        )
        .execute(db)
        .await?;

        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period IN ('trend_week', 'trend_month') \
             AND period_key < date('now', '-2 days')",
        )
        .execute(db)
        .await?;
        progress.set(72.0);

        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT source, external_id, 'weekly', strftime('%Y-W%W', period_key), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'daily' \
             GROUP BY source, external_id, strftime('%Y-W%W', period_key)",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'daily' AND period_key < date('now', '-14 days')",
        )
        .execute(db)
        .await?;
        progress.set(76.0);

        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT source, external_id, 'monthly', strftime('%Y-%m', period_key), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'weekly' \
             GROUP BY source, external_id, strftime('%Y-%m', period_key)",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'weekly' AND period_key < date('now', '-56 days')",
        )
        .execute(db)
        .await?;
        progress.set(84.0);

        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT source, external_id, 'yearly', strftime('%Y', period_key), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'monthly' \
             GROUP BY source, external_id, strftime('%Y', period_key)",
        )
        .execute(db)
        .await?;
        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'monthly' AND period_key < date('now', '-730 days')",
        )
        .execute(db)
        .await?;
        progress.set(92.0);

        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (source, external_id, period, period_key, avg, min, max, sample_count) \
             SELECT source, external_id, 'all', 'all', \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'monthly' \
             GROUP BY source, external_id",
        )
        .execute(db)
        .await?;

        info!("popularity data refresh complete");
        progress.set(100.0);
        Ok(())
    }
}
