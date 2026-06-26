use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{error, info};

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct SnapshotMetricsTask;

#[async_trait]
impl Task for SnapshotMetricsTask {
    fn key(&self) -> &str {
        "SnapshotMetrics"
    }

    fn name(&self) -> &str {
        "Snapshot Popularity Metrics"
    }

    fn description(&self) -> &str {
        "Fetches the current popularity score for every item in the library from enabled metric addons and writes a daily snapshot to popularity_raw."
    }

    fn short_description(&self) -> &str {
        "Snapshots per-item popularity scores"
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
        let addons = ctx
            .addons
            .metrics_addons();
        let total = addons.len();

        if total == 0 {
            progress.set(100.0);
            return Ok(());
        }

        for (i, runtime) in addons
            .iter()
            .enumerate()
        {
            let sub = progress.step(i, total);
            let addon = runtime
                .metrics
                .as_ref()
                .unwrap();

            info!(addon = %runtime.row.name, "snapshotting metrics");
            match addon
                .snapshot_metrics(&ctx, sub)
                .await
            {
                Ok(snapshots) => {
                    let count = snapshots.len();
                    if let Err(e) = bulk_insert_snapshots(&ctx, &snapshots).await {
                        error!(addon = %runtime.row.name, error = %e, "failed to insert metric snapshots");
                    } else {
                        info!(addon = %runtime.row.name, count, "metric snapshots written");
                    }
                }
                Err(e) => {
                    error!(addon = %runtime.row.name, error = %e, "snapshot_metrics failed");
                }
            }
        }

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
