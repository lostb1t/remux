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
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        const CHUNK_SIZE: u32 = 100;
        let mut offset = 0u32;
        loop {
            let batch = sqlx::query_as::<_, db::Media>(
                r#"
        SELECT *
        FROM media
        WHERE kind IN (?, ?, ?, ?)
        ORDER BY id
        LIMIT ? OFFSET ?
        "#,
            )
            .bind(db::MediaKind::Movie)
            .bind(db::MediaKind::Series)
            .bind(db::MediaKind::Season)
            .bind(db::MediaKind::Episode)
            .bind(CHUNK_SIZE)
            .bind(offset)
            .fetch_all(&ctx.db)
            .await?;

            if batch.is_empty() {
                break;
            }
            let fetched = batch.len() as u32;
            ctx.addons
                .process_meta_batch(batch, &ctx, false, true)
                .await?;
            if fetched < CHUNK_SIZE {
                break;
            }
            offset += CHUNK_SIZE;
        }
        Ok(())
    }
}
