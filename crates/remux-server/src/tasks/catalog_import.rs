use anyhow::Result;
use async_trait::async_trait;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use tracing::{debug, error, info, warn};

use super::catalog_import_shared::{catalog_membership_tag, import_catalog_items};
use super::{ProgressReporter, Task, TaskService};
use crate::addons::make_media_id;
use crate::{AppContext, db};

pub struct CatalogImportTask;

#[async_trait]
impl Task for CatalogImportTask {
    fn key(&self) -> &str {
        "CatalogImport"
    }
    fn name(&self) -> &str {
        "Import Catalogs"
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
        let global_max = db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let addons = ctx.addons.catalog_addons().await;
        let total_work = addons.len().max(1);
        let mut valid_tags: HashSet<String> = HashSet::new();
        let mut total_counts: HashMap<String, usize> = HashMap::new();

        for (addon_idx, runtime) in addons.iter().enumerate() {
            let addon_id = runtime.row.id;
            let catalog_states = runtime.row.catalog_states();

            let prefix = format!("addon:{addon_id}:");

            let available = match runtime.kind.catalog_list(&ctx).await {
                Ok(v) => v,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to list addon catalogs, skipping");
                    continue;
                }
            };

            let enabled: Vec<_> = available
                .iter()
                .filter(|cat_info| {
                    let local_id = &cat_info.provider_catalog_id;
                    catalog_states
                        .get(local_id.as_str())
                        .map(|s| s.enabled)
                        .unwrap_or(false)
                })
                .collect();

            debug!(
                addon = %addon_id,
                total = available.len(),
                enabled = enabled.len(),
                "importing enabled catalogs"
            );

            for (cat_idx, cat_info) in enabled.iter().enumerate() {
                let pct = (addon_idx * 100 + cat_idx * 100 / enabled.len().max(1))
                    as f64
                    / total_work as f64;
                progress.set(pct);

                let full_id = make_media_id(addon_id, &cat_info.provider_catalog_id);
                let local_id = full_id.strip_prefix(&prefix).unwrap_or(&full_id);
                let max = catalog_states
                    .get(local_id)
                    .and_then(|s| s.max_items)
                    .map(|n| n as usize)
                    .unwrap_or(global_max);

                if let Some(tag) = catalog_membership_tag(&full_id) {
                    valid_tags.insert(tag);
                }

                let source = match ctx.addons.make_catalog_stream(&full_id).await {
                    Some(s) => s,
                    None => {
                        warn!(catalog = %full_id, "no addon found for catalog, skipping");
                        continue;
                    }
                };

                debug!(catalog = %full_id, max, "importing catalog items");

                let stream = match source.stream(&ctx).await {
                    Ok(s) => s,
                    Err(e) => {
                        error!(catalog = %full_id, error = %e, "failed to open catalog stream");
                        continue;
                    }
                };

                let counts = import_catalog_items(
                    &ctx.db,
                    uuid::Uuid::nil(),
                    &full_id,
                    max,
                    stream,
                    &progress,
                )
                .await?;

                debug!(catalog = %full_id, ?counts, "import complete");

                for (kind, n) in counts {
                    *total_counts.entry(kind).or_insert(0) += n;
                }
            }
        }

        // Remove tags for catalogs that no longer exist or are no longer enabled.
        // We batch-delete in chunks to stay within SQLite's variable limit.
        let total: usize = total_counts.values().sum();
        let mut sorted: Vec<_> = total_counts.iter().collect();
        sorted.sort_by_key(|(k, _)| k.as_str());
        let breakdown: Vec<String> =
            sorted.iter().map(|(k, v)| format!("{}={}", k, v)).collect();
        info!(total, counts = %breakdown.join(" "), "catalog import complete");

        remove_stale_catalog_tags(&ctx.db, &valid_tags).await;

        tasks.run_task("RefreshLibrary").await?;
        Ok(())
    }
}

/// Delete `catalog:*` tags from `media_tags` that are not in `valid_tags`.
pub(crate) async fn remove_stale_catalog_tags(
    db: &sqlx::SqlitePool,
    valid_tags: &HashSet<String>,
) {
    // Fetch all distinct catalog tags currently in the DB.
    let existing: Vec<String> = match sqlx::query_scalar(
        "SELECT DISTINCT tag FROM media_tags WHERE tag LIKE 'catalog:%'",
    )
    .fetch_all(db)
    .await
    {
        Ok(v) => v,
        Err(e) => {
            warn!(error = %e, "failed to fetch catalog tags for cleanup");
            return;
        }
    };

    let stale: Vec<&String> = existing
        .iter()
        .filter(|t| !valid_tags.contains(*t))
        .collect();

    if stale.is_empty() {
        return;
    }

    info!(count = stale.len(), "removing stale catalog tags");
    for tag in stale {
        if let Err(e) = sqlx::query("DELETE FROM media_tags WHERE tag = ?")
            .bind(tag)
            .execute(db)
            .await
        {
            warn!(tag = %tag, error = %e, "failed to remove stale catalog tag");
        }
    }
}
