use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::sync::Arc;
use tracing::{error, info, warn};

use crate::{AppContext, db};
use super::{ProgressReporter, Task, TaskService};

pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn key(&self) -> &str { "CatalogImport" }
    fn name(&self) -> &str { "Catalog Import" }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        // Prefer ctx.aio (long-lived, has a warm in-memory cache) so cached
        // manifest/catalog responses are reused across task runs.  Only fall
        // back to building a fresh client when ctx.aio is None, which happens
        // when the AIO URL was configured after startup via the setup wizard.
        let aio = if let Some(ref existing) = ctx.aio {
            existing.clone()
        } else {
            let url = crate::db::Settings::get_config(&ctx.db).await.ok()
                .and_then(|c| c.aio_url)
                .filter(|s| !s.is_empty())
                .unwrap_or_default();
            if url.is_empty() {
                anyhow::bail!("AIO URL not configured — complete the setup wizard first");
            }
            crate::aio::AioService::from_url(&url)?
        };

        let catalog_max_items: usize = crate::db::Settings::get_config(&ctx.db).await.ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(5000) as usize;

        let manifest = aio.get_manifest().await?;
        let mut total_imported = 0;

        let catalogs: Vec<_> = manifest.catalogs.into_iter()
            .filter(|c| !c.id.contains("search"))
            .collect();
        let total = catalogs.len();

        info!("starting catalog import ({} catalogs)", total);

        for (i, cat) in catalogs.into_iter().enumerate() {
            progress.set(i as f64 / total as f64 * 100.0);
            info!("importing catalog {} {}", cat.id, cat.kind);

            let aio_id = format!("{}:{}", cat.kind, cat.id);
            let mut media_cat = db::Media::get_by_filter(
                &ctx.db,
                &db::MediaFilter {
                    aio_id: Some(aio_id.clone()),
                    ..Default::default()
                },
            )
            .await?
            .records
            .first()
            .cloned()
            .unwrap_or_else(|| db::Media {
                title: cat.name.clone(),
                kind: db::MediaKind::Collection,
                aio_id: Some(aio_id),
                collection_kind: Some(db::CollectionKind::Manual),
                ..Default::default()
            });

            media_cat.save(&ctx.db).await?;

            let mut meta_stream = aio.get_catalog_stream(&cat).await.chunks(500);
            let mut count = 0;

            while let Some(mut metas) = meta_stream.next().await {
                let remaining = catalog_max_items.saturating_sub(count);
                metas = metas.into_iter().take(remaining).collect();

                let items: Vec<db::Media> = metas
                    .into_iter()
                    .unique_by(|meta| meta.id.clone())
                    .flat_map(|meta| match db::aio_meta_to_medias(meta) {
                        Ok(items) => items.into_iter(),
                        Err(e) => {
                            warn!(error = %e, "failed to convert metadata, skipping");
                            Vec::<db::Media>::new().into_iter()
                        }
                    })
                    .collect();

                if items.is_empty() {
                    break;
                }

                if let Err(e) = db::Media::insert(&ctx.db, &items).await {
                    error!("failed to import chunk: {}", e);
                } else {
                    count += items.len();
                    total_imported += count;
                }

                if count >= catalog_max_items {
                    break;
                }
            }

            info!("finished importing catalog {} {} ({} items)", cat.id, cat.kind, count);
        }

        info!("import complete, total: {}", total_imported);

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}
