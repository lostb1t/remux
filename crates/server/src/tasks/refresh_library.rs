use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::{
    AppContext, db,
    providers::{AioMetaProvider, AioTreeSyncProvider, MetaProviderService},
};

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
        _progress: ProgressReporter,
    ) -> Result<()> {
        let service = MetaProviderService::new(
            vec![Box::new(AioMetaProvider)],
            vec![Box::new(AioTreeSyncProvider)],
        );
        const CHUNK_SIZE: u32 = 500;
        let mut offset = 0u32;
        loop {
            let batch = db::Media::get_refreshable(&ctx.db, CHUNK_SIZE, offset).await?;
            if batch.is_empty() {
                break;
            }
            let fetched = batch.len() as u32;
            let updated = service.process(batch, &ctx).await?;
            db::Media::upsert(&ctx.db, &updated).await?;
            if fetched < CHUNK_SIZE {
                break;
            }
            offset += CHUNK_SIZE;
        }
        Ok(())
    }
}
