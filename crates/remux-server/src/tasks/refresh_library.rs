use anyhow::Result;
use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{
    ProgressReporter, Task, TaskService,
    catalog_import_shared::{
        catalog_membership, import_catalog_items, remove_stale_catalog_memberships,
    },
};
use crate::{AppContext, addons::make_media_id, db};

pub struct RefreshLibraryTask;

#[async_trait]
impl Task for RefreshLibraryTask {
    fn key(&self) -> &str {
        "RefreshLibrary"
    }
    fn name(&self) -> &str {
        "Refresh Library"
    }
    fn description(&self) -> &str {
        "Imports catalogs, scans addon sources, and updates the media library index."
    }
    fn short_description(&self) -> &str {
        "Syncs all addon catalogs into your library"
    }
    fn category(&self) -> &str {
        "Library"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let global_max = db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        ctx.addons
            .refresh_indexes(&ctx, progress.scaled(0.0, 20.0))
            .await?;

        let addons = ctx
            .addons
            .catalog_addons();
        let total_work = addons
            .len()
            .max(1);
        let mut valid_pairs: HashSet<(String, String)> = HashSet::new();

        let catalog_progress = progress.scaled(20.0, 70.0);
        for (addon_idx, runtime) in addons
            .iter()
            .enumerate()
        {
            let addon_progress = catalog_progress.step(addon_idx, total_work);
            let addon_id = runtime
                .row
                .id;
            let catalog_states = runtime
                .row
                .catalog_states();
            let prefix = format!("addon:{addon_id}:");

            let available = match runtime
                .catalog
                .as_ref()
                .expect("catalog_addons() guarantees catalog slot")
                .catalog_list(&ctx)
                .await
            {
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
                        .unwrap_or(cat_info.default_enabled)
                })
                .collect();

            debug!(
                addon = %addon_id,
                total = available.len(),
                enabled = enabled.len(),
                "importing enabled catalogs"
            );

            for (cat_idx, cat_info) in enabled
                .iter()
                .enumerate()
            {
                addon_progress.report(
                    cat_idx,
                    enabled
                        .len()
                        .max(1),
                );

                let full_id = make_media_id(addon_id, &cat_info.provider_catalog_id);
                let local_id = full_id
                    .strip_prefix(&prefix)
                    .unwrap_or(&full_id);
                let max = catalog_states
                    .get(local_id)
                    .and_then(|s| s.max_items)
                    .or(cat_info.default_max_items)
                    .map(|n| n as usize)
                    .unwrap_or(global_max);

                if let Some((addon_uuid, local_cat_id)) = catalog_membership(&full_id) {
                    valid_pairs
                        .insert((addon_uuid.to_string(), local_cat_id.to_string()));
                }

                let source = match ctx
                    .addons
                    .make_catalog_stream(&full_id)
                {
                    Some(s) => s,
                    None => {
                        warn!(catalog = %full_id, "no addon found for catalog, skipping");
                        continue;
                    }
                };

                debug!(catalog = %full_id, max, "importing catalog items");

                let stream = match source
                    .stream(&ctx)
                    .await
                {
                    Ok(s) => s,
                    Err(e) => {
                        error!(catalog = %full_id, error = %e, "failed to open catalog stream");
                        continue;
                    }
                };

                let counts = import_catalog_items(
                    &ctx,
                    Uuid::nil(),
                    &full_id,
                    max,
                    stream,
                    &addon_progress,
                )
                .await?;

                info!(catalog = %full_id, ?counts, "catalog import complete");

                // Sync collection relations if this catalog has a linked collection.
                if catalog_states
                    .get(local_id)
                    .map(|s| s.create_collection)
                    .unwrap_or(false)
                {
                    let collection_id = Uuid::new_v5(&addon_id, local_id.as_bytes());
                    let addon_id_str = addon_id.to_string();
                    let media_ids: Vec<Uuid> = sqlx::query_scalar(
                        "SELECT media_id FROM media_catalog_items WHERE addon_id = ? AND catalog_id = ?",
                    )
                    .bind(&addon_id_str)
                    .bind(local_id)
                    .fetch_all(&ctx.db)
                    .await
                    .unwrap_or_default();

                    if let Err(e) = db::MediaRelation::replace_collection_items(
                        &ctx.db,
                        &collection_id,
                        &media_ids,
                    )
                    .await
                    {
                        warn!(catalog = %full_id, error = %e, "failed to sync catalog collection relations");
                    }

                    // Backfill collection_media_kind for collections created before this field existed.
                    if let Some(kind) = &cat_info.collection_media_kind {
                        sqlx::query(
                            "UPDATE media SET collection_media_kind = ? \
                             WHERE id = ? AND collection_media_kind IS NULL",
                        )
                        .bind(kind)
                        .bind(collection_id)
                        .execute(&ctx.db)
                        .await
                        .ok();
                    }
                }
            }
        }

        remove_stale_catalog_memberships(&ctx.db, &valid_pairs).await;

        const CHUNK_SIZE: u32 = 100;
        let mut total: Option<u32> = None;
        let mut processed = 0u32;
        let mut offset = 0u32;
        let meta_progress = progress.scaled(70.0, 100.0);
        loop {
            let (batch, count) = db::Media::get_refreshable(
                &ctx.db,
                CHUNK_SIZE,
                offset,
                total.is_none(),
            )
            .await?;
            if let Some(c) = count {
                total = Some(c.max(1));
            }
            if batch.is_empty() {
                break;
            }
            let fetched = batch.len() as u32;
            ctx.addons
                .process_meta_batch(batch, &ctx, false)
                .await?;
            processed += fetched;
            if let Some(t) = total {
                meta_progress.report(processed as usize, t as usize);
            }
            if fetched < CHUNK_SIZE {
                break;
            }
            offset += CHUNK_SIZE;
        }
        Ok(())
    }
}
