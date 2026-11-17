use anyhow::Result;
use async_trait::async_trait;
use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};
use tracing::info;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct ClearCacheTask;

#[async_trait]
impl Task for ClearCacheTask {
    fn key(&self) -> &str {
        "ClearCache"
    }
    fn name(&self) -> &str {
        "Clear Cache"
    }
    fn description(&self) -> &str {
        "Clears the in-memory metadata store and the HTTP response cache, and deletes all cached images and extracted subtitles and fonts."
    }
    fn short_description(&self) -> &str {
        "Clears all caches"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        ctx.store
            .clear();
        crate::sdks::clear_http_cache();
        progress.set(50.0);
        // Everything but a `.tmp` an extraction is still writing.
        let dirs = super::clean_cache_directory::disk_cache_dirs(
            &ctx.config
                .data_dir,
        );
        let (removed, bytes) = tokio::task::spawn_blocking(move || {
            dirs.iter()
                .map(|dir| {
                    super::clean_cache_directory::clean_old_files(
                        dir,
                        SystemTime::now(),
                        Duration::ZERO,
                        super::clean_cache_directory::MAX_TMP_AGE,
                    )
                })
                .fold((0, 0), |(n, b), (dn, db)| (n + dn, b + db))
        })
        .await?;
        info!(removed, bytes, "cleared disk caches");
        progress.set(100.0);
        Ok(())
    }
}
