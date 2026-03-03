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
        let media_list = db::Media::get_refreshable(&ctx.db).await?;
        let updated_media = service.process(media_list, &ctx).await?;
        db::Media::upsert(&ctx.db, &updated_media).await?;
        Ok(())
    }
}
