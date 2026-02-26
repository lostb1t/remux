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
        let manifest = ctx.aio.get_manifest().await?;
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
                kind: db::MediaKind::Catalog,
                aio_id: Some(aio_id),
                catalog_kind: Some(db::CatalogKind::Manual),
                ..Default::default()
            });

            media_cat.save(&ctx.db).await?;

            let mut meta_stream = ctx.aio.get_catalog_stream(&cat).await.chunks(500);
            let mut count = 0;

            while let Some(mut metas) = meta_stream.next().await {
                let remaining = ctx.config.catalog_max_items.saturating_sub(count);
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

                if count >= ctx.config.catalog_max_items {
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
