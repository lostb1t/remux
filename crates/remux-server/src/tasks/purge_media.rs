use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct PurgeMediaTask;

const PURGE_KINDS: &str = "'movie','series','season','episode','source','track','album','artist','person','genre'";

#[async_trait]
impl Task for PurgeMediaTask {
    fn key(&self) -> &str {
        "PurgeMedia"
    }
    fn name(&self) -> &str {
        "Purge Library"
    }
    fn description(&self) -> &str {
        "Wipes all imported media from the database."
    }
    fn short_description(&self) -> &str {
        "Wipes all imported media from the database (no physical files are deleted)."
    }
    fn category(&self) -> &str {
        "Maintenance"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        // Checkpoint the WAL before bulk deletes to reduce WAL traversal overhead.
        sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
            .execute(&ctx.db)
            .await
            .ok();

        // Each pre-delete runs in its own auto-commit transaction so the write lock
        // is released between steps — other writers (devices heartbeat, etc.) can
        // proceed rather than waiting for the full 10-second purge.
        sqlx::query(&format!(
            "DELETE FROM media_tags WHERE media_id IN \
             (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!(
            "DELETE FROM media_images WHERE media_id IN \
             (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!(
            "DELETE FROM media_relations WHERE \
             left_media_id  IN (SELECT id FROM media WHERE kind IN ({PURGE_KINDS})) OR \
             right_media_id IN (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!(
            "DELETE FROM media_catalog_items WHERE media_id IN \
             (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&ctx.db)
        .await?;

        sqlx::query(&format!("DELETE FROM media WHERE kind IN ({PURGE_KINDS})"))
            .execute(&ctx.db)
            .await?;

        ctx.addons.purge_indexes(&ctx).await?;

        Ok(())
    }
}
