use anyhow::Result;
use async_trait::async_trait;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::trace;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
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
    fn category(&self) -> TaskCategory {
        TaskCategory::Library
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        const CHUNK_SIZE: u32 = 100;

        let task_started = std::time::Instant::now();
        let count_started = std::time::Instant::now();
        let total: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE kind IN (?, ?, ?, ?)")
                .bind(db::MediaKind::Movie)
                .bind(db::MediaKind::Series)
                .bind(db::MediaKind::Artist)
                .bind(db::MediaKind::Album)
                .fetch_one(&ctx.db)
                .await?;
        let total = total as usize;
        trace!(
            target: "remux_server::metadata_refresh",
            total,
            elapsed = ?count_started.elapsed(),
            chunk_size = CHUNK_SIZE,
            "starting full metadata refresh"
        );

        // Shared counter incremented per item inside process_meta_batch so progress
        // updates as each concurrent item finishes, not once per full 100-item batch.
        let processed = Arc::new(AtomicUsize::new(0));
        let on_item_done: Arc<dyn Fn() + Send + Sync> = {
            let processed = Arc::clone(&processed);
            let progress = progress.clone();
            let total = total.max(1);
            Arc::new(move || {
                let n = processed.fetch_add(1, Ordering::Relaxed) + 1;
                progress.report(n, total);
            })
        };

        // Cursor-based pagination: WHERE id > last_id guarantees forward progress even
        // when refresh fails and refreshed_at is not updated for an item.
        let mut last_id: Option<uuid::Uuid> = None;
        let mut batches = 0usize;
        loop {
            let fetch_started = std::time::Instant::now();
            let batch = if let Some(cursor) = last_id {
                sqlx::query_as::<_, db::Media>(
                    "SELECT * FROM media WHERE kind IN (?, ?, ?, ?) AND id > ? ORDER BY id LIMIT ?",
                )
                .bind(db::MediaKind::Movie)
                .bind(db::MediaKind::Series)
                .bind(db::MediaKind::Artist)
                .bind(db::MediaKind::Album)
                .bind(cursor)
                .bind(CHUNK_SIZE)
                .fetch_all(&ctx.db)
                .await?
            } else {
                sqlx::query_as::<_, db::Media>(
                    "SELECT * FROM media WHERE kind IN (?, ?, ?, ?) ORDER BY id LIMIT ?",
                )
                .bind(db::MediaKind::Movie)
                .bind(db::MediaKind::Series)
                .bind(db::MediaKind::Artist)
                .bind(db::MediaKind::Album)
                .bind(CHUNK_SIZE)
                .fetch_all(&ctx.db)
                .await?
            };

            if batch.is_empty() {
                break;
            }
            batches += 1;
            let batch_len = batch.len();
            let fetch_elapsed = fetch_started.elapsed();
            trace!(
                target: "remux_server::metadata_refresh",
                batch = batches,
                items = batch_len,
                processed = processed.load(Ordering::Relaxed),
                fetch_elapsed = ?fetch_elapsed,
                "full metadata refresh batch starting"
            );
            last_id = batch
                .last()
                .map(|m| m.id);
            let process_started = std::time::Instant::now();
            ctx.addons
                .process_meta_batch(batch, &ctx, true, Some(Arc::clone(&on_item_done)))
                .await?;
            trace!(
                target: "remux_server::metadata_refresh",
                batch = batches,
                items = batch_len,
                processed = processed.load(Ordering::Relaxed),
                fetch_elapsed = ?fetch_elapsed,
                process_elapsed = ?process_started.elapsed(),
                "full metadata refresh batch complete"
            );
        }
        trace!(
            target: "remux_server::metadata_refresh",
            total,
            processed = processed.load(Ordering::Relaxed),
            batches,
            elapsed = ?task_started.elapsed(),
            "full metadata refresh complete"
        );
        Ok(())
    }
}
