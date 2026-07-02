use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct PurgeMetricsTask;

#[async_trait]
impl Task for PurgeMetricsTask {
    fn key(&self) -> &str {
        "PurgeMetrics"
    }

    fn name(&self) -> &str {
        "Purge Metrics Data"
    }

    fn description(&self) -> &str {
        "Deletes all popularity snapshots and aggregated trend data from the database. Run this to reset metric history before re-ingesting fresh data."
    }

    fn short_description(&self) -> &str {
        "Clears all popularity snapshots and trend aggregates"
    }

    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
    }
    fn destructive(&self) -> bool {
        true
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        sqlx::query("DELETE FROM popularity_raw")
            .execute(&ctx.db)
            .await?;
        progress.set(50.0);

        sqlx::query("DELETE FROM popularity_agg")
            .execute(&ctx.db)
            .await?;

        info!("metrics data purged");
        progress.set(100.0);
        Ok(())
    }
}
