use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
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
        "Clears the in-memory metadata store and the HTTP response cache."
    }
    fn short_description(&self) -> &str {
        "Clears in-memory and HTTP caches"
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
        ctx.store.clear();
        progress.set(50.0);
        crate::sdks::clear_http_cache();
        progress.set(100.0);
        Ok(())
    }
}
