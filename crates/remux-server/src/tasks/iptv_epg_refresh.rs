use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db, iptv};

pub struct IptvEpgRefreshTask;

#[async_trait]
impl Task for IptvEpgRefreshTask {
    fn key(&self) -> &str {
        "RefreshIptvEpg"
    }
    fn name(&self) -> &str {
        "Refresh IPTV EPG"
    }
    fn description(&self) -> &str {
        "Fetches programme guide data for all configured IPTV sources."
    }
    fn short_description(&self) -> &str {
        "Fetches EPG data from all IPTV sources"
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
        let addons: Vec<_> = ctx
            .addons
            .list()
            .iter()
            .filter(|r| r.supports_type(&db::MediaKind::TvChannel))
            .map(|r| {
                (
                    r.row
                        .id,
                    r.row
                        .preset
                        .kind
                        .clone(),
                    r.row
                        .preset
                        .config
                        .clone(),
                )
            })
            .collect();

        if addons.is_empty() {
            progress.set(100.0);
            return Ok(());
        }

        let client = reqwest::Client::new();

        for (idx, (addon_id, kind, config)) in addons
            .iter()
            .enumerate()
        {
            progress.report(idx, addons.len());

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
            match iptv::stream_import_epg(&client, &epg_url, &channel_refs, &ctx).await
            {
                Ok(count) => info!(addon = %addon_id, programs = count, "imported EPG"),
                Err(e) => warn!(addon = %addon_id, error = %e, "failed to fetch EPG"),
            }
        }

        progress.set(100.0);
        Ok(())
    }
}
