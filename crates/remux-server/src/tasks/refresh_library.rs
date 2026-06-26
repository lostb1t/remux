use anyhow::Result;
use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{
    ProgressReporter, Task, TaskService,
    catalog_import_shared::{
        import_catalog_items, prune_orphaned_playlists,
        remove_stale_catalog_memberships,
    },
};
use crate::{AppContext, db};

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
        let global_max = db::Settings::get_config_or_default(&ctx.db)
            .await
            .catalog_max_items
            .unwrap_or(250) as usize;

        ctx.addons
            .refresh_indexes(&ctx, progress.scaled(0.0, 20.0))
            .await?;

        // Keep in sync with the top-level kinds import_catalog_items() actually
        // persists (catalog_import_shared.rs's retain filter), minus TvChannel
        // (owned by RefreshIptv).
        const LIBRARY_KINDS: &[db::MediaKind] = &[
            db::MediaKind::Movie,
            db::MediaKind::Series,
            db::MediaKind::Artist,
            db::MediaKind::Album,
            db::MediaKind::Track,
            db::MediaKind::Playlist,
        ];
        let addons = ctx
            .addons
            .catalogs_for_kinds(&ctx, LIBRARY_KINDS)
            .await;
        let total_work = addons
            .len()
            .max(1);
        let mut valid_collection_ids: HashSet<Uuid> = HashSet::new();
        let mut domain_collection_ids: HashSet<Uuid> = HashSet::new();

        let catalog_progress = progress.scaled(20.0, 70.0);
        for (addon_idx, (runtime, available)) in addons
            .iter()
            .enumerate()
        {
            let addon_progress = catalog_progress.step(addon_idx, total_work);
            let addon_id = runtime
                .row
                .id;

            for cat_info in available {
                domain_collection_ids.insert(cat_info.collection_id);
            }

            let enabled: Vec<_> = available
                .iter()
                .filter(|cat_info| cat_info.enabled)
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

                let full_id = &cat_info.catalog_id;
                let max = cat_info
                    .max_items
                    .map(|n| n as usize)
                    .unwrap_or(global_max);

                valid_collection_ids.insert(cat_info.collection_id);

                let source = match ctx
                    .addons
                    .make_catalog_stream(full_id)
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

                let (counts, new_counts) = import_catalog_items(
                    &ctx,
                    cat_info,
                    full_id,
                    max,
                    stream,
                    &addon_progress,
                )
                .await?;

                info!(catalog = %full_id, total = ?counts, new = ?new_counts, "catalog import complete");
            }
        }

        // Must run before remove_stale_catalog_memberships below.
        prune_orphaned_playlists(
            &ctx.db,
            &valid_collection_ids,
            &domain_collection_ids,
        )
        .await;
        remove_stale_catalog_memberships(
            &ctx.db,
            &valid_collection_ids,
            &domain_collection_ids,
        )
        .await;

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
