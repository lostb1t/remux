use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{error, info, warn};

use super::catalog_import_shared::import_catalog_items;
use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, aio, db};

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

        let global_max = db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let tmdb_client = crate::common::tmdb_client(&ctx.db).await;

        let aio_svc = aio::AioService::from_settings(&ctx.db).await.ok();

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

            let stream = match provider.stream_items(provider_catalog_id, &ctx).await {
                Ok(s) => s,
                Err(e) => {
                    error!(catalog = media_id, error = %e, "failed to open catalog stream");
                    continue;
                }
            };

            let count = import_catalog_items(
                &ctx.db,
                catalog.id,
                media_id,
                max,
                stream,
                aio_svc.as_ref(),
                tmdb_client.as_ref(),
                &progress,
            )
            .await?;

            info!(catalog = media_id, count, "import complete");
        }

        tasks.run_task("RefreshLibrary").await?;
        Ok(())
    }
}
