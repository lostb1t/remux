use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct PurgeMediaTask;

const PURGE_KINDS: &str =
    "'movie','series','season','episode','source','track','album','artist'";

#[async_trait]
impl Task for PurgeMediaTask {
    fn key(&self) -> &str {
        "PurgeMedia"
    }
    fn name(&self) -> &str {
        "Purge Media"
    }
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        let mut tx = ctx.db.begin().await?;

        // Pre-delete from child tables in bulk before touching `media`.
        // Relying on ON DELETE CASCADE fires one delete per parent row into these
        // tables, which is very slow for large libraries even with indexes.
        // Doing it explicitly here is a single bulk scan each.
        sqlx::query(&format!(
            "DELETE FROM media_tags WHERE media_id IN \
             (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&mut *tx)
        .await?;

        sqlx::query(&format!(
            "DELETE FROM media_relations WHERE \
             left_media_id  IN (SELECT id FROM media WHERE kind IN ({PURGE_KINDS})) OR \
             right_media_id IN (SELECT id FROM media WHERE kind IN ({PURGE_KINDS}))"
        ))
        .execute(&mut *tx)
        .await?;

        // Now the cascade machinery (parent_id self-ref, grandparent_id check) has
        // almost no child-table work left — this runs fast.
        sqlx::query(&format!("DELETE FROM media WHERE kind IN ({PURGE_KINDS})"))
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}
