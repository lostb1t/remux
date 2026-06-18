use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::{collections::HashSet, sync::Arc};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use super::{
    ProgressReporter, Task, TaskService,
    catalog_import_shared::{
        import_catalog_items, prune_stale_iptv_channels,
        remove_stale_catalog_memberships,
    },
};
use crate::{AppContext, db};

pub struct RefreshIptvTask;

#[async_trait]
impl Task for RefreshIptvTask {
    fn key(&self) -> &str {
        "RefreshIptv"
    }
    fn name(&self) -> &str {
        "Refresh IPTV"
    }
    fn description(&self) -> &str {
        "Imports channel catalogs and fetches programme guide data for all configured IPTV sources."
    }
    fn short_description(&self) -> &str {
        "Syncs channels and EPG data from all IPTV sources"
    }
    fn category(&self) -> &str {
        "Live TV"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let iptv_runtimes = ctx
            .addons
            .catalogs_for_kinds(&ctx, &[db::MediaKind::TvChannel])
            .await;

        let global_max = db::Settings::get_config(&ctx.db)
            .await
            .ok()
            .and_then(|c| c.catalog_max_items)
            .unwrap_or(250) as usize;

        let total_work = iptv_runtimes
            .len()
            .max(1);
        let mut valid_collection_ids: HashSet<Uuid> = HashSet::new();
        let mut domain_collection_ids: HashSet<Uuid> = HashSet::new();
        let import_start = Utc::now().naive_utc();

        let catalog_progress = progress.scaled(0.0, 50.0);
        for (addon_idx, (runtime, available)) in iptv_runtimes
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
                "importing enabled IPTV catalogs"
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

                debug!(catalog = %full_id, max, "importing IPTV catalog items");

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

                info!(catalog = %full_id, total = ?counts, new = ?new_counts, "IPTV catalog import complete");
            }
        }

        remove_stale_catalog_memberships(
            &ctx.db,
            &valid_collection_ids,
            &domain_collection_ids,
        )
        .await;
        prune_stale_iptv_channels(&ctx.db, import_start).await;

        let epg_progress = progress.scaled(50.0, 100.0);
        let client = reqwest::Client::new();

        for (idx, (runtime, _)) in iptv_runtimes
            .iter()
            .enumerate()
        {
            epg_progress.report(idx, iptv_runtimes.len());

            let addon_id = runtime
                .row
                .id;
            let kind = &runtime
                .row
                .preset
                .kind;
            let config = &runtime
                .row
                .preset
                .config;

            let epg_url = if kind == "iptv-xtream" {
                let server_url = config["server_url"]
                    .as_str()
                    .unwrap_or("")
                    .trim_end_matches('/');
                let user = config["username"]
                    .as_str()
                    .unwrap_or("");
                let pass = config["password"]
                    .as_str()
                    .unwrap_or("");
                if server_url.is_empty() || user.is_empty() {
                    continue;
                }
                format!("{server_url}/xmltv.php?username={user}&password={pass}")
            } else {
                match config["epg_url"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                {
                    Some(u) => u.to_string(),
                    None => continue,
                }
            };

            let source_id = addon_id
                .simple()
                .to_string();
            let channel_refs: Vec<(Uuid, Option<String>)> = sqlx::query_as(
                "SELECT id, tvg_id FROM media \
                 WHERE kind = 'tv_channel' \
                   AND json_extract(external_ids, '$.iptv_source_id') = ? \
                   AND enabled = TRUE",
            )
            .bind(&source_id)
            .fetch_all(&ctx.db)
            .await
            .unwrap_or_default();

            if channel_refs.is_empty() {
                debug!(addon = %addon_id, "no channels found for EPG import, skipping");
                continue;
            }

            debug!(addon = %addon_id, channels = channel_refs.len(), url = %epg_url, "fetching EPG");
            match crate::iptv::stream_import_epg(&client, &epg_url, &channel_refs, &ctx)
                .await
            {
                Ok(count) => info!(addon = %addon_id, programs = count, "imported EPG"),
                Err(e) => warn!(addon = %addon_id, error = %e, "failed to fetch EPG"),
            }
        }

        progress.set(100.0);
        Ok(())
    }
}
