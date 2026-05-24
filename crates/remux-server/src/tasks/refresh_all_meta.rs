use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};

pub struct RefreshAllMetaTask;

#[async_trait]
impl Task for RefreshAllMetaTask {
    fn key(&self) -> &str {
        "RefreshAllMeta"
    }
    fn name(&self) -> &str {
        "Refresh All Metadata"
    }
    fn description(&self) -> &str {
        "Fetches metadata (artwork, ratings, etc.) for all library items."
    }
    fn short_description(&self) -> &str {
        "Re-fetches artwork and info for all items"
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
        const CHUNK_SIZE: u32 = 100;

        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE kind IN (?, ?)")
                .bind(db::MediaKind::Movie)
                .bind(db::MediaKind::Series)
                .fetch_one(&ctx.db)
                .await?;
        let total = total as usize;

        let mut processed = 0usize;
        let mut offset = 0u32;
        loop {
            let batch = sqlx::query_as::<_, db::Media>(
                "SELECT * FROM media WHERE kind IN (?, ?) ORDER BY id LIMIT ? OFFSET ?",
            )
            .bind(db::MediaKind::Movie)
            .bind(db::MediaKind::Series)
            .bind(CHUNK_SIZE)
            .bind(offset)
            .fetch_all(&ctx.db)
            .await?;

            if batch.is_empty() {
                break;
            }
            let fetched = batch.len();
            ctx.addons.process_meta_batch(batch, &ctx, false).await?;
            processed += fetched;
            progress.report(processed, total.max(1));
            if fetched < CHUNK_SIZE as usize {
                break;
            }
            offset += CHUNK_SIZE;
        }
        Ok(())
    }
}
