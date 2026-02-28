use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{AppContext, db};
use super::{ProgressReporter, Task, TaskService};

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
    fn key(&self) -> &str { &self.key }
    fn name(&self) -> &str { &self.display_name }
    fn category(&self) -> &str { "Library" }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let aio = crate::aio::AioService::from_settings(&ctx.db).await?;

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

        let aio_id = catalog.aio_id.as_deref()
            .ok_or_else(|| anyhow::anyhow!("Catalog has no aio_id"))?
            .to_string();

        let manifest = aio.get_manifest().await?;
        let manifest_cat = manifest.catalogs.iter()
            .find(|c| format!("{}:{}", c.kind, c.id) == aio_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Catalog {} not found in AIO manifest", aio_id))?;

        let global_max = crate::db::Settings::get_config(&ctx.db).await.ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let max = catalog.collection_max_items
            .map(|n| n as usize)
            .unwrap_or(global_max);

        info!("importing catalog {} (max={})", aio_id, max);

        let catalog_id = catalog.id;
        let mut meta_stream = aio.get_catalog_stream(&manifest_cat).await.chunks(500);
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
                if let Err(e) = db::MediaRelation::upsert(&ctx.db, &relations).await {
                    error!("failed to upsert catalog relations: {}", e);
                }
            }

            count += items.len();
            if count >= max {
                break;
            }
        }

        info!("import complete for catalog {}: {} items", aio_id, count);

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}
