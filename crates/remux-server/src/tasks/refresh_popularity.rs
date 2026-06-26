use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{error, info};

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
        "Fetches the latest popularity score for every item in your library and updates the historical trend data used for sorting by popularity."
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
        let addons = ctx
            .addons
            .metrics_addons();

        if !addons.is_empty() {
            let total = addons.len();
            for (i, runtime) in addons
                .iter()
                .enumerate()
            {
                let sub = progress
                    .scaled(0.0, 60.0)
                    .step(i, total);
                let addon = runtime
                    .metrics
                    .as_ref()
                    .unwrap();

                info!(addon = %runtime.row.name, "fetching popularity scores");
                match addon
                    .snapshot_metrics(&ctx, sub)
                    .await
                {
                    Ok(snapshots) => {
                        let count = snapshots.len();
                        if let Err(e) = bulk_insert_snapshots(&ctx, &snapshots).await {
                            error!(addon = %runtime.row.name, error = %e, "failed to write popularity scores");
                        } else {
                            info!(addon = %runtime.row.name, count, "popularity scores written");
                        }
                    }
                    Err(e) => {
                        error!(addon = %runtime.row.name, error = %e, "failed to fetch popularity scores");
                    }
                }
            }
        }

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

async fn bulk_insert_snapshots(
    ctx: &AppContext,
    snapshots: &[crate::addons::MetricSnapshot],
) -> Result<()> {
    if snapshots.is_empty() {
        return Ok(());
    }
    for chunk in snapshots.chunks(400) {
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT INTO popularity_raw (source, external_id, value, date) ",
        );
        qb.push_values(chunk, |mut b, s| {
            b.push_bind(&s.source)
                .push_bind(&s.external_id)
                .push_bind(s.value)
                .push_bind(&s.date);
        });
        qb.push(" ON CONFLICT DO UPDATE SET value = excluded.value");
        qb.build()
            .execute(&ctx.db)
            .await?;
    }
    Ok(())
}
