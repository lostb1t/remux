use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct ClearImageCacheTask;

#[async_trait]
impl Task for ClearImageCacheTask {
    fn key(&self) -> &str {
        "ClearImageCache"
    }
    fn name(&self) -> &str {
        "Clear Image Cache"
    }
    fn description(&self) -> &str {
        "Deletes all cached images from the image cache directory."
    }
    fn short_description(&self) -> &str {
        "Deletes all cached images"
    }
    fn category(&self) -> &str {
        "Maintenance"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let cache_dir = ctx.config.data_dir.join("cache").join("images");
        let mut removed = 0usize;

        for entry in super::iter_dir(&cache_dir) {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                tracing::warn!(path = %entry.path().display(), error = %e, "failed to remove cached image");
            } else {
                removed += 1;
            }
        }

        info!(removed, "cleared image cache");
        progress.set(100.0);
        Ok(())
    }
}
