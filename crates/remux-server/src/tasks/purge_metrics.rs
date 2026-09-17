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
        "Purge Metrics"
    }

    fn description(&self) -> &str {
        "Deletes all RemuxDB popularity and trending metrics from the database."
    }

    fn short_description(&self) -> &str {
        "Clears all RemuxDB media metrics"
    }

    fn category(&self) -> TaskCategory {
        TaskCategory::Purge
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
        sqlx::query("DELETE FROM media_metrics")
            .execute(&ctx.db)
            .await?;

        info!("media metrics purged");
        progress.set(100.0);
        Ok(())
    }
}
