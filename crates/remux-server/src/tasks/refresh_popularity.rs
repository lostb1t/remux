use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct RefreshPopularityTask;

#[async_trait]
impl Task for RefreshPopularityTask {
    fn key(&self) -> &str {
        "RefreshPopularity"
    }

    fn name(&self) -> &str {
        "Fetch Daily Metrics"
    }

    fn description(&self) -> &str {
        "Fetches the latest popularity score for every item in your library and updates the historical trend data used for sorting by popularity. Run this task daily for accurate trending results."
    }

    fn short_description(&self) -> &str {
        "Updates popularity scores and trend history"
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
        // --- Phase 1: fetch fresh scores from all metrics addons (0–60%) ---
        ctx.addons
            .snapshot_all_metrics(&ctx, progress.scaled(0.0, 60.0))
            .await?;
        progress.set(60.0);

        // --- Phase 2: roll up into trend buckets (60–100%) ---
        let db = &ctx.db;

        // Daily: average across all sources for the same media item.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT media_id, 'daily', date, \
                    AVG(value), MIN(value), MAX(value), COUNT(*) \
             FROM popularity_raw \
             WHERE media_id IS NOT NULL \
             GROUP BY media_id, date",
        )
        .execute(db)
        .await?;
        sqlx::query("DELETE FROM popularity_raw WHERE date < date('now', '-2 days')")
            .execute(db)
            .await?;
        progress.set(68.0);

        // trend_week: weighted trend = recent_avg * MIN(ratio, 5.0).
        // Splits available daily rows into two halves: last 3 days vs prior 3 days.
        // Multiplying ratio by the absolute score prevents low-popularity noise items
        // from dominating on a large ratio alone. Ratio is capped at 5× so that
        // near-zero → small jumps don't explode. Falls back to recent_avg when no
        // earlier window exists yet.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT new.media_id, 'trend_week', date('now'), \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    1 \
             FROM ( \
               SELECT media_id, AVG(avg) AS avg \
               FROM popularity_agg \
               WHERE period = 'daily' \
                 AND period_key BETWEEN date('now', '-2 days') AND date('now') \
               GROUP BY media_id \
             ) new \
             LEFT JOIN ( \
               SELECT media_id, AVG(avg) AS avg \
               FROM popularity_agg \
               WHERE period = 'daily' \
                 AND period_key BETWEEN date('now', '-5 days') AND date('now', '-3 days') \
               GROUP BY media_id \
             ) old ON old.media_id = new.media_id",
        )
        .execute(db)
        .await?;
        progress.set(70.0);

        // Weekly rollup runs before trend_month so that today's daily data is included
        // in the weekly aggregates that trend_month uses.
        // period_key is the Monday of the week so that strftime calls on it work correctly.
        // 'weekday 0' advances to the next Sunday; '-6 days' steps back to Monday.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT media_id, \
                    'weekly', \
                    date(period_key, 'weekday 0', '-6 days'), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'daily' \
             GROUP BY media_id, date(period_key, 'weekday 0', '-6 days')",
        )
        .execute(db)
        .await?;
        progress.set(72.0);

        // trend_month: weighted trend = recent_avg * MIN(ratio, 5.0).
        // Splits available weekly rows into two halves: last 2 weeks vs prior 2 weeks.
        // Same weighted formula with 5× ratio cap as trend_week.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT new.media_id, 'trend_month', date('now'), \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    CASE WHEN old.avg > 0 THEN new.avg * MIN(new.avg / old.avg, 5.0) ELSE new.avg END, \
                    1 \
             FROM ( \
               SELECT media_id, AVG(avg) AS avg \
               FROM popularity_agg \
               WHERE period = 'weekly' \
                 AND period_key >= date('now', '-13 days') \
               GROUP BY media_id \
             ) new \
             LEFT JOIN ( \
               SELECT media_id, AVG(avg) AS avg \
               FROM popularity_agg \
               WHERE period = 'weekly' \
                 AND period_key BETWEEN date('now', '-27 days') AND date('now', '-14 days') \
               GROUP BY media_id \
             ) old ON old.media_id = new.media_id",
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
        sqlx::query(
            "DELETE FROM popularity_agg \
             WHERE period = 'daily' AND period_key < date('now', '-14 days')",
        )
        .execute(db)
        .await?;
        progress.set(76.0);

        // Monthly period_key is 'YYYY-MM'. The weekly period_key is a real date
        // so strftime('%Y-%m', period_key) works correctly here.
        // The IS NOT NULL guard defends against any stale old-format rows.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT media_id, 'monthly', strftime('%Y-%m', period_key), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'weekly' \
               AND strftime('%Y-%m', period_key) IS NOT NULL \
             GROUP BY media_id, strftime('%Y-%m', period_key)",
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

        // Yearly period_key is 'YYYY'. Monthly period_key is 'YYYY-MM' which is
        // not a parseable SQLite date, so substr is used instead of strftime.
        sqlx::query(
            "INSERT OR REPLACE INTO popularity_agg \
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT media_id, 'yearly', substr(period_key, 1, 4), \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'monthly' \
             GROUP BY media_id, substr(period_key, 1, 4)",
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
             (media_id, period, period_key, avg, min, max, sample_count) \
             SELECT media_id, 'all', 'all', \
                    AVG(avg), MIN(min), MAX(max), SUM(sample_count) \
             FROM popularity_agg WHERE period = 'monthly' \
             GROUP BY media_id",
        )
        .execute(db)
        .await?;

        // Refresh the latest flag: clear and re-mark the most recent period_key
        // per (media_id, period).  This lets the sort query use a simple
        // `AND pop.latest = 1` instead of a GROUP BY + MAX subquery.
        for period in &["daily", "weekly", "monthly", "trend_week", "trend_month"] {
            sqlx::query("UPDATE popularity_agg SET latest = 0 WHERE period = ?1")
                .bind(period)
                .execute(db)
                .await?;
            sqlx::query(
                "UPDATE popularity_agg SET latest = 1 \
                 WHERE period = ?1 \
                   AND (media_id, period_key) IN (\
                     SELECT media_id, MAX(period_key) \
                     FROM popularity_agg \
                     WHERE period = ?1 \
                     GROUP BY media_id\
                   )",
            )
            .bind(period)
            .execute(db)
            .await?;
        }

        info!("popularity data refresh complete");
        progress.set(100.0);
        Ok(())
    }
}
