use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct RollupPopularityTask;

#[async_trait]
impl Task for RollupPopularityTask {
    fn key(&self) -> &str {
        "RollupPopularity"
    }

    fn name(&self) -> &str {
        "Roll Up Popularity Stats"
    }

    fn description(&self) -> &str {
        "Aggregates daily popularity snapshots into weekly, monthly, yearly, and all-time buckets, then prunes the raw data."
    }

    fn short_description(&self) -> &str {
        "Rolls up popularity snapshots into aggregated buckets"
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
        let db = &ctx.db;

        // --- raw → daily ---
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
        info!("popularity: raw → daily done");
        progress.set(10.0);

        sqlx::query("DELETE FROM popularity_raw WHERE date < date('now', '-2 days')")
            .execute(db)
            .await?;
        progress.set(15.0);

        // --- daily → weekly ---
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
        info!("popularity: daily → weekly done");
        progress.set(30.0);

        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'daily' AND period_key < date('now', '-14 days')",
        )
        .execute(db)
        .await?;
        progress.set(35.0);

        // --- weekly → monthly ---
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
        info!("popularity: weekly → monthly done");
        progress.set(55.0);

        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'weekly' AND period_key < date('now', '-56 days')",
        )
        .execute(db)
        .await?;
        progress.set(60.0);

        // --- monthly → yearly ---
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
        info!("popularity: monthly → yearly done");
        progress.set(80.0);

        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'monthly' AND period_key < date('now', '-730 days')",
        )
        .execute(db)
        .await?;
        progress.set(85.0);

        // --- all-time ---
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
        info!("popularity: all-time updated");

        progress.set(100.0);
        Ok(())
    }
}
