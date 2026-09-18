use anyhow::Result;
use async_trait::async_trait;
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{
    ProgressReporter, Task, TaskCategory, TaskService,
    catalog_import_shared::{
        import_catalog_items, prune_orphaned_playlists,
        remove_stale_catalog_memberships,
    },
};
use crate::{AppContext, common::ItemProgress, db, services::image::ImageService};
use remux_sdks::stremio::ResourceType;

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
    fn category(&self) -> TaskCategory {
        TaskCategory::Library
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
        let mut valid_collection_ids: HashSet<Uuid> = HashSet::new();
        let mut domain_collection_ids: HashSet<Uuid> = HashSet::new();

        // Flatten to (catalog, expected item count) up front so each catalog's
        // share of the progress range is weighted by how much work it's actually
        // expected to do, rather than split evenly per addon/catalog. An equal
        // split let a catalog that skips or fails instantly (disabled addon,
        // broken stream) consume just as much of the bar as one that imports
        // thousands of items — several of those landing back to back could jump
        // the bar forward well before any real work happened.
        let mut work: Vec<(crate::addons::ResolvedCatalog, usize)> = Vec::new();
        for (runtime, available) in &addons {
            let addon_id = runtime
                .row
                .id;
            for cat_info in available {
                domain_collection_ids.insert(cat_info.collection_id);
            }
            if !runtime
                .row
                .resources
                .contains(&ResourceType::Catalog)
            {
                continue;
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
            for cat_info in enabled {
                let max = cat_info
                    .max_items
                    .map(|n| n as usize)
                    .unwrap_or(global_max);
                work.push((cat_info.clone(), max));
            }
        }
        let total_catalog_items: usize = work
            .iter()
            .map(|(_, max)| *max)
            .sum();

        // Size the overall progress total from all three phases' best-available
        // estimates up front, as a single running item count instead of the
        // old fixed 0-20/20-70/70-100 percentage split — a phase that's fast
        // or has nothing to do (an addon with no new files, a catalog with no
        // new items) no longer eats a disproportionate, structurally-fixed
        // share of the bar regardless of its actual size. The index and
        // metadata-refresh estimates get corrected against their real counts
        // as soon as each phase learns them (see `refresh_indexes` and the
        // metadata loop below); the catalog estimate is each catalog's own
        // configured max_items, already a real number rather than a guess.
        let index_estimate = ctx
            .addons
            .estimate_index_items(&ctx)
            .await;
        let (_, refreshable_count) =
            db::Media::get_refreshable(&ctx.db, 0, None, true).await?;
        let refreshable_estimate = refreshable_count.unwrap_or(0) as usize;
        let grand_total =
            (index_estimate + total_catalog_items + refreshable_estimate).max(1);
        let item_progress = ItemProgress::new(progress, grand_total);

        let index_actual = ctx
            .addons
            .refresh_indexes(&ctx, &item_progress, 0)
            .await?;
        let catalog_base = index_actual;

        // `max` is each catalog's configured cap, not its real size — a
        // catalog that returns far fewer items (or nothing, on a skip/error)
        // would otherwise consume its whole estimated slice regardless of
        // actual work done. `item_progress.adjust_total` corrects that once
        // the real count is known, same as the index-refresh phase above;
        // `cumulative` advances by the real count too, so later catalogs'
        // base offsets aren't thrown off by earlier ones' estimates.
        let mut cumulative: usize = 0;
        for (cat_info, max) in &work {
            let catalog_item_progress =
                item_progress.child(catalog_base + cumulative, *max);

            let full_id = &cat_info.catalog_id;
            valid_collection_ids.insert(cat_info.collection_id);

            let source = match ctx
                .addons
                .make_catalog_stream(full_id)
            {
                Some(s) => s,
                None => {
                    warn!(catalog = %full_id, "no addon found for catalog, skipping");
                    catalog_item_progress.set(100.0);
                    item_progress.adjust_total(-(*max as i64));
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
                    catalog_item_progress.set(100.0);
                    item_progress.adjust_total(-(*max as i64));
                    continue;
                }
            };

            let (counts, new_counts) = import_catalog_items(
                &ctx,
                cat_info,
                full_id,
                *max,
                stream,
                &catalog_item_progress,
            )
            .await?;

            catalog_item_progress.set(100.0);

            let real: usize = counts
                .values()
                .sum();
            item_progress.adjust_total(real as i64 - *max as i64);
            cumulative += real;

            info!(catalog = %full_id, total = ?counts, new = ?new_counts, "catalog import complete");
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

        // `cumulative`, not `total_catalog_items`, holds the real post-correction
        // count now that each catalog's estimate has been reconciled above.
        let meta_base = catalog_base + cumulative;
        const CHUNK_SIZE: u32 = 100;
        let mut total: Option<u32> = None;
        let mut processed = 0u32;
        let mut last_id: Option<Uuid> = None;
        // Created once the real refreshable count is known (see below) —
        // replaces the upfront `refreshable_estimate` guess so this phase's
        // slice of `item_progress` reflects reality instead of a stale count
        // taken before catalog import may have added new items to refresh.
        let mut meta_progress: Option<crate::common::ProgressReporter> = None;
        info!("starting metadata refresh pass");
        let meta_start = std::time::Instant::now();
        loop {
            let (batch, count) = db::Media::get_refreshable(
                &ctx.db,
                CHUNK_SIZE,
                last_id,
                total.is_none(),
            )
            .await?;

            if let Some(c) = count {
                total = Some(c.max(1));
                info!(total = c, "refreshable items found");
                item_progress.adjust_total(c as i64 - refreshable_estimate as i64);
                meta_progress = Some(item_progress.child(meta_base, c.max(1) as usize));
            }
            if batch.is_empty() {
                break;
            }
            let fetched = batch.len() as u32;
            last_id = batch
                .last()
                .map(|m| m.id);
            let batch_start = std::time::Instant::now();
            ctx.addons
                .process_meta_batch(batch, &ctx, false, None)
                .await?;
            processed += fetched;
            debug!(
                target: "remux_server::metadata_refresh",
                fetched,
                processed,
                total = ?total,
                elapsed = ?batch_start.elapsed(),
                "metadata batch processed"
            );
            if let (Some(t), Some(mp)) = (total, &meta_progress) {
                mp.report(processed as usize, t as usize);
            }
            if fetched < CHUNK_SIZE {
                break;
            }
        }
        // Guarantees this phase's slice always reaches its end, even when
        // there was nothing to refresh at all (the loop then never runs a
        // batch, so `meta_progress` would otherwise sit at 0% of its slice).
        if let Some(mp) = &meta_progress {
            mp.set(100.0);
        }
        info!(elapsed = ?meta_start.elapsed(), processed, "metadata refresh pass complete");

        // Bust the generated poster cache for all collections so the grid
        // reflects any newly imported items on the next request.
        invalidate_collection_images(&ctx).await;

        Ok(())
    }
}

/// Page size for `invalidate_collection_images`'s scan. Without an explicit
/// `limit`, `get_by_filter` appends no LIMIT clause at all — every page (this
/// one included) would return the entire collections table in one query.
const INVALIDATE_PAGE_SIZE: u32 = 500;

async fn invalidate_collection_images(ctx: &AppContext) {
    let filter = db::MediaFilter {
        kind: Some(vec![db::MediaKind::Collection]),
        limit: Some(INVALIDATE_PAGE_SIZE),
        total_count: true,
        ..Default::default()
    };
    let Ok(result) = db::Media::get_by_filter(&ctx.db, &filter).await else {
        return;
    };
    for col in &result.records {
        // Only invalidate generated images for collections that use the image
        // configurator. Collections with no image_config may have a manually
        // uploaded image that must not be deleted.
        if col
            .collection_image_config
            .is_none()
        {
            continue;
        }
        let _ = ImageService::invalidate_collection_image(
            &ctx.config
                .data_dir,
            col.id,
            &ctx.db,
        )
        .await;
    }
    if result.total_count
        > result
            .records
            .len()
    {
        // There are more collections; page through them.
        let mut offset = result
            .records
            .len() as u32;
        loop {
            let filter = db::MediaFilter {
                kind: Some(vec![db::MediaKind::Collection]),
                limit: Some(INVALIDATE_PAGE_SIZE),
                offset: Some(offset),
                ..Default::default()
            };
            let Ok(page) = db::Media::get_by_filter(&ctx.db, &filter).await else {
                break;
            };
            for col in &page.records {
                if col
                    .collection_image_config
                    .is_none()
                {
                    continue;
                }
                let _ = ImageService::invalidate_collection_image(
                    &ctx.config
                        .data_dir,
                    col.id,
                    &ctx.db,
                )
                .await;
            }
            offset += page
                .records
                .len() as u32;
            if page
                .records
                .is_empty()
                || offset as usize >= result.total_count
            {
                break;
            }
        }
    }
}
