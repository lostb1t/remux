use anyhow::Result;
use async_trait::async_trait;
use futures::stream::StreamExt;
use itertools::Itertools;
use std::collections::HashSet;
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
        "Import All"
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
        let all_db_catalogs = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Catalog]),
                ..Default::default()
            },
        )
        .await?
        .records;

        for provider in ctx.catalogs.providers() {
            let pid = provider.provider_id();
            let available = match provider.list_catalogs(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(provider = pid, error = %e, "failed to list catalogs, skipping provider");
                    continue;
                }
            };

            let prefix = format!("{}:", pid);
            let provider_db: Vec<_> = all_db_catalogs
                .iter()
                .filter(|c| {
                    c.media_id
                        .as_deref()
                        .map(|id| id.starts_with(&prefix))
                        .unwrap_or(false)
                })
                .collect();

            let available_ids: HashSet<String> = available
                .iter()
                .map(|c| format!("{}:{}", pid, c.provider_catalog_id))
                .collect();

            // Delete catalogs that no longer exist in the provider.
            for stale in provider_db.iter().filter(|c| {
                !available_ids.contains(c.media_id.as_deref().unwrap_or(""))
            }) {
                info!(
                    catalog = stale.media_id.as_deref().unwrap_or("?"),
                    "removing stale catalog"
                );
                if let Err(e) = db::Media::delete(&ctx.db, &stale.id).await {
                    error!(catalog = %stale.id, error = %e, "failed to delete stale catalog");
                }
            }

            // Insert catalogs not yet in the DB (disabled by default).
            for cat_info in &available {
                let full_id = format!("{}:{}", pid, cat_info.provider_catalog_id);
                if provider_db
                    .iter()
                    .any(|d| d.media_id.as_deref() == Some(&full_id))
                {
                    continue;
                }
                info!(catalog = %full_id, "registering new catalog");
                let mut media = db::Media {
                    kind: db::MediaKind::Catalog,
                    title: cat_info.name.clone(),
                    media_id: Some(full_id),
                    promoted: false,
                    ..Default::default()
                };
                if let Err(e) = media.save(&ctx.db).await {
                    error!(catalog = %cat_info.name, error = %e, "failed to register new catalog");
                }
            }
        }

        let enabled_catalogs = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Catalog]),
                promoted: Some(true),
                ..Default::default()
            },
        )
        .await?
        .records;

        info!(
            "importing items from {} enabled catalogs",
            enabled_catalogs.len()
        );

        let global_max = crate::db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        for (i, catalog) in enabled_catalogs.iter().enumerate() {
            progress.set(i as f64 / enabled_catalogs.len().max(1) as f64 * 100.0);

            let media_id = match catalog.media_id.as_deref() {
                Some(id) => id,
                None => {
                    warn!(catalog = %catalog.id, "catalog has no media_id, skipping");
                    continue;
                }
            };

            let provider = match ctx.catalogs.provider_for_media_id(media_id) {
                Some(p) => p,
                None => {
                    warn!(
                        catalog = media_id,
                        "no provider found for catalog, skipping"
                    );
                    continue;
                }
            };

            let provider_catalog_id = ctx.catalogs.strip_prefix(provider, media_id);
            let max = catalog
                .collection_max_items
                .map(|n| n as usize)
                .unwrap_or(global_max);

            info!(catalog = media_id, max, "importing catalog items");

            let catalog_id = catalog.id;
            let mut meta_stream = match provider
                .stream_items(provider_catalog_id, &ctx)
                .await
            {
                Ok(s) => s.chunks(500),
                Err(e) => {
                    error!(catalog = media_id, error = %e, "failed to open catalog stream");
                    continue;
                }
            };

            let mut count = 0usize;
            while let Some(mut metas) = meta_stream.next().await {
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
                    error!(catalog = media_id, error = %e, "failed to import chunk");
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
                    if let Err(e) = db::MediaRelation::upsert(&ctx.db, &relations).await
                    {
                        error!(catalog = media_id, error = %e, "failed to upsert catalog relations");
                    }
                }

                count += items.iter().filter(|m| m.parent_id.is_none()).count();
                if count >= max {
                    break;
                }
            }

            info!(catalog = media_id, count, "import complete");
        }

        tasks.run_task("RefreshLibrary").await?;
        Ok(())
    }
}
