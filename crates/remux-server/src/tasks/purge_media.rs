use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct PurgeMediaTask;

#[async_trait]
impl Task for PurgeMediaTask {
    fn key(&self) -> &str {
        "PurgeMedia"
    }
    fn name(&self) -> &str {
        "Purge Media"
    }
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM media WHERE kind IN ('movie','series','season','episode','source')",
        )
        .execute(&ctx.db)
        .await?;
        Ok(())
    }
}
