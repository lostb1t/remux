use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};

pub struct SeriesSyncTask;

#[async_trait]
impl Task for SeriesSyncTask {
    fn key(&self) -> &str {
        "SeriesSync"
    }
    fn name(&self) -> &str {
        "Series Sync"
    }
    fn description(&self) -> &str {
        "Syncs series episode data across configured sources."
    }
    fn short_description(&self) -> &str {
        "Updates episode lists for all series"
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
        let media_list = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Series]),
                ..Default::default()
            },
        )
        .await?
        .records;

        ctx.addons
            .process_meta_batch(media_list, &ctx, false)
            .await?;

        Ok(())
    }
}
