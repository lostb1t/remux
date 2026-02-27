use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::sync::Arc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::{AppContext, db};
use super::{ProgressReporter, Task, TaskService};

pub struct CollectionImportTask {
    pub collection_id: Uuid,
    key: String,
    display_name: String,
}

impl CollectionImportTask {
    pub fn new(collection_id: Uuid, name: impl AsRef<str>) -> Self {
        Self {
            collection_id,
            key: Self::key_for(collection_id),
            display_name: format!("Import: {}", name.as_ref()),
        }
    }

    pub fn key_for(collection_id: Uuid) -> String {
        format!("catalog_import:{}", collection_id)
    }
}

#[async_trait]
impl Task for CollectionImportTask {
    fn key(&self) -> &str { &self.key }
    fn name(&self) -> &str { &self.display_name }
    fn category(&self) -> &str { "Collections" }

    async fn run(
        &self,
        ctx: AppContext,
        tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
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

        let collection = db::Media::get_by_id(&ctx.db, &self.collection_id)
            .await?
            .ok_or_else(|| anyhow!("Collection {} not found", self.collection_id))?;

        let aio_id = collection.aio_id
            .ok_or_else(|| anyhow!("Collection has no AIO catalog mapped"))?;

        // Per-collection limit takes priority, then fall back to global setting.
        let catalog_max_items: usize = match collection.collection_max_items {
            Some(n) => n as usize,
            None => crate::db::Settings::get_config(&ctx.db).await.ok()
                .and_then(|c| c.catalog_max_items)
                .unwrap_or(250) as usize,
        };

        let manifest = aio.get_manifest().await?;
        let catalog = manifest.catalogs.into_iter()
            .find(|c| format!("{}:{}", c.kind, c.id) == aio_id)
            .ok_or_else(|| anyhow!("AIO catalog '{}' not found in manifest", aio_id))?;

        info!("importing catalog {} for collection {}", aio_id, self.collection_id);

        let collection_id = self.collection_id;
        let mut meta_stream = aio.get_catalog_stream(&catalog).await.chunks(500);
        let mut count = 0;

        while let Some(mut metas) = meta_stream.next().await {
            progress.set(count as f64 / catalog_max_items.max(1) as f64 * 100.0);

            let remaining = catalog_max_items.saturating_sub(count);
            if remaining == 0 {
                break;
            }
            metas = metas.into_iter().take(remaining).collect();

            let items: Vec<db::Media> = metas
                .into_iter()
                .unique_by(|meta| meta.id.clone())
                .flat_map(|meta| match db::aio_meta_to_medias(meta) {
                    Ok(mut items) => {
                        // Link the top-level item (movie/series) to this collection.
                        if let Some(top) = items.first_mut() {
                            top.parent_id = Some(collection_id);
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
            } else {
                count += items.len();
            }

            if count >= catalog_max_items {
                break;
            }
        }

        info!("import complete for collection {}: {} items", self.collection_id, count);

        tasks.run_task("RefreshLibrary").await?;

        Ok(())
    }
}
