use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct PurgeMusicTask;

#[async_trait]
impl Task for PurgeMusicTask {
    fn key(&self) -> &str {
        "PurgeMusic"
    }
    fn name(&self) -> &str {
        "Purge Music"
    }
    fn description(&self) -> &str {
        "Wipes all tracks, albums, and artists from the database."
    }
    fn short_description(&self) -> &str {
        "Removes all music items (no physical files are deleted)."
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
        _progress: ProgressReporter,
    ) -> Result<()> {
        super::purge_shared::purge_by_kinds(&ctx, &["track", "album", "artist"]).await
    }
}
