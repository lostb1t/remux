use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::sync::Arc;
use tracing::{error, info, warn};

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};

pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn key(&self) -> &str {
        "CatalogImport"
    }
    fn name(&self) -> &str {
        "Catalog Import"
    }
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let aio = crate::aio::AioService::from_settings(&ctx.db).await?;

        // Fetch all enabled catalog media items (kind=catalog, promoted=1)
        let catalogs = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Catalog]),
                promoted: Some(true),
                ..Default::default()
            },
        )
        .await?
        .records;

        info!("found {} enabled catalogs to import", catalogs.len());

        let manifest = aio.get_manifest().await?;

        let global_max = crate::db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        // Pair each enabled catalog with its manifest entry upfront
        let pairs: Vec<(db::Media, crate::sdks::aio::Catalog)> = catalogs
            .into_iter()
            .filter_map(|cat| {
                let aio_id = cat.aio_id.as_deref()?.to_string();
                let manifest_cat = manifest
                    .catalogs
                    .iter()
                    .find(|c| format!("{}:{}", c.kind, c.id) == aio_id)?
                    .clone();
                Some((cat, manifest_cat))
            })
            .collect();

        info!("{} catalogs matched in manifest", pairs.len());

        for (catalog, manifest_cat) in pairs {
            let aio_id = catalog.aio_id.as_deref().unwrap_or("?");
            let max = catalog
                .collection_max_items
                .map(|n| n as usize)
                .unwrap_or(global_max);

            info!("importing catalog {} (max={})", aio_id, max);

            let catalog_id = catalog.id;
            let mut meta_stream =
                aio.get_catalog_stream(&manifest_cat).await.chunks(500);
            let mut count = 0usize;

            while let Some(mut metas) = meta_stream.next().await {
                progress.set(count as f64 / max.max(1) as f64 * 100.0);

                let remaining = max.saturating_sub(count);
                if remaining == 0 {
                    break;
                }
                metas = metas.into_iter().take(remaining).collect();

                let items: Vec<db::Media> = metas
                    .into_iter()
                    .unique_by(|meta| meta.id.clone())
                    .flat_map(|meta| match db::aio_meta_to_medias(meta) {
                        Ok(mut items) => {
                            // Top-level item floats freely — no parent_id
                            if let Some(top) = items.first_mut() {
                                top.parent_id = None;
                            }
                            items.into_iter()
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to convert metadata, skipping");
                            Vec::<db::Media>::new().into_iter()
                        }
                    })
                    .collect();

                if items.is_empty() {
                    break;
                }

                if let Err(e) = db::Media::upsert(&ctx.db, &items).await {
                    error!("failed to import chunk: {}", e);
                    continue;
                }

                // Link each top-level item to this catalog via media_relations
                let relations: Vec<db::MediaRelation> = items
                    .iter()
                    .filter(|m| m.parent_id.is_none())
                    .map(|m| db::MediaRelation {
                        left_media_id: m.id,
                        right_media_id: catalog_id,
                        role: Some(db::RelationRole::Catalog),
                        ..Default::default()
                    })
                    .collect();

                if !relations.is_empty() {
                    if let Err(e) = db::MediaRelation::upsert(&ctx.db, &relations).await
                    {
                        error!("failed to upsert catalog relations: {}", e);
                    }
                }

                count += items.len();
                if count >= max {
                    break;
                }
            }

            info!("import complete for catalog {}: {} items", aio_id, count);
        }

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}
