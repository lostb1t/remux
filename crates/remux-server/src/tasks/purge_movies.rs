use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct PurgeMoviesTask;

#[async_trait]
impl Task for PurgeMoviesTask {
    fn key(&self) -> &str {
        "PurgeMovies"
    }
    fn name(&self) -> &str {
        "Purge Movies"
    }
    fn description(&self) -> &str {
        "Wipes all movies from the database."
    }
    fn short_description(&self) -> &str {
        "Removes all movie items (no physical files are deleted)."
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
        super::purge_shared::purge_by_kinds(&ctx, &["movie"]).await
    }
}
