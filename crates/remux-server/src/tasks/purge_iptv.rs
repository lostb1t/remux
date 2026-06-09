use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct PurgeIptvTask;

#[async_trait]
impl Task for PurgeIptvTask {
    fn key(&self) -> &str {
        "PurgeIptv"
    }
    fn name(&self) -> &str {
        "Purge IPTV"
    }
    fn description(&self) -> &str {
        "Wipes all IPTV channels and programs from the database."
    }
    fn short_description(&self) -> &str {
        "Removes all TV channels and programs (no physical files are deleted)."
    }
    fn category(&self) -> &str {
        "Live TV"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        sqlx::query(
            "DELETE FROM media
             WHERE kind = 'tv_program'
               AND parent_id IN (SELECT id FROM media WHERE kind = 'tv_channel')",
        )
        .execute(&ctx.db)
        .await?;

        sqlx::query("DELETE FROM media WHERE kind = 'tv_channel'")
            .execute(&ctx.db)
            .await?;

        ctx.addons
            .purge_indexes(&ctx)
            .await?;

        Ok(())
    }
}
