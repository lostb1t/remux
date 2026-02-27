use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;

use crate::{AppContext, db};
use super::{CollectionImportTask, ProgressReporter, Task, TaskService};

pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn key(&self) -> &str { "CatalogImport" }
    fn name(&self) -> &str { "Collection Import" }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        let collections = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Collection]),
                ..Default::default()
            },
        )
        .await?
        .records;

        let catalog_collections: Vec<_> = collections
            .into_iter()
            .filter(|m| m.collection_kind == Some(db::CollectionKind::Catalog))
            .collect();

        info!("kicking off {} catalog collection imports", catalog_collections.len());

        for col in catalog_collections {
            let key = CollectionImportTask::key_for(col.id);
            tasks.run_task(&key).await?;
        }

        Ok(())
    }
}
