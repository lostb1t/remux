use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::info;
use uuid::Uuid;

use super::catalog_import_shared::import_catalog_items;
use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, aio, db};

pub struct CatalogItemImportTask {
    catalog_id: Uuid,
    key: String,
    display_name: String,
}

impl CatalogItemImportTask {
    pub fn new(catalog_id: Uuid, name: &str) -> Self {
        Self {
            catalog_id,
            key: Self::task_key(catalog_id),
            display_name: format!("Import {}", name),
        }
    }

    pub fn task_key(catalog_id: Uuid) -> String {
        format!("catalogimport:{}", catalog_id)
    }
}

#[async_trait]
impl Task for CatalogItemImportTask {
    fn key(&self) -> &str {
        &self.key
    }
    fn name(&self) -> &str {
        &self.display_name
    }
    fn category(&self) -> &str {
        "Import"
    }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let tmdb_client = crate::common::tmdb_client(&ctx.db).await;

        let catalog = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                id: Some(vec![self.catalog_id]),
                kind: Some(vec![db::MediaKind::Catalog]),
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Catalog {} not found", self.catalog_id))?;

        let media_id = catalog
            .media_id
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("Catalog has no media_id"))?
            .to_string();

        let provider =
            ctx.catalogs
                .provider_for_media_id(&media_id)
                .ok_or_else(|| {
                    anyhow::anyhow!("No provider found for catalog {}", media_id)
                })?;

        let provider_catalog_id =
            ctx.catalogs.strip_prefix(provider, &media_id).to_string();

        let global_max = db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let max = catalog
            .collection_max_items
            .map(|n| n as usize)
            .unwrap_or(global_max);

        let aio_svc = aio::AioService::from_settings(&ctx.db).await?;

        info!(catalog = %media_id, max, "importing catalog items");

        let stream = provider.stream_items(&provider_catalog_id, &ctx).await?;

        let count = import_catalog_items(
            &ctx.db,
            catalog.id,
            &media_id,
            max,
            stream,
            Some(&aio_svc),
            tmdb_client.as_ref(),
            &progress,
        )
        .await?;

        info!(catalog = %media_id, count, "import complete");

        tasks.run_task("RefreshLibrary").await?;
        Ok(())
    }
}
