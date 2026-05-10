use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};

pub struct RefreshLibraryTask;

#[async_trait]
impl Task for RefreshLibraryTask {
    fn key(&self) -> &str {
        "RefreshLibrary"
    }
    fn name(&self) -> &str {
        "Refresh Library"
    }
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        // Phase 1 (0-50%): refresh addon file indexes
        ctx.addons
            .refresh_indexes(&ctx, progress.scaled(0.0, 50.0))
            .await?;

        // Phase 2 (50-100%): refresh metadata for all stale media
        const CHUNK_SIZE: u32 = 100;
        let mut total: Option<u32> = None;
        let mut processed = 0u32;
        let mut offset = 0u32;
        let meta_progress = progress.scaled(50.0, 100.0);
        loop {
            let (batch, count) = db::Media::get_refreshable(
                &ctx.db,
                CHUNK_SIZE,
                offset,
                total.is_none(),
            )
            .await?;
            if let Some(c) = count {
                total = Some(c.max(1));
            }
            if batch.is_empty() {
                break;
            }
            let fetched = batch.len() as u32;
            ctx.addons
                .process_meta_batch(batch, &ctx, false, true)
                .await?;
            processed += fetched;
            if let Some(t) = total {
                meta_progress.report(processed as usize, t as usize);
            }
            if fetched < CHUNK_SIZE {
                break;
            }
            offset += CHUNK_SIZE;
        }
        Ok(())
    }
}
