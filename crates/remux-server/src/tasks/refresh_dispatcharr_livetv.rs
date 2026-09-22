use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, error, info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{
    AppContext,
    addons::dispatcharr::{
        DispatcharrChannel, channel_to_media, channel_versions, fetch_channel_streams,
        fetch_channels, fetch_epg_data, fetch_epg_grid,
    },
    db,
};

/// Syncs Dispatcharr channel rows, their stream versions, their EPG and their
/// recordings. Standalone: it does not require `RefreshIptvTask` to have run.
/// Channel-stream requests in flight at once against one Dispatcharr.
const CHANNEL_STREAM_CONCURRENCY: usize = 8;

/// Channel ids per batched statement, under SQLite's bind-variable limit.
const SQL_BIND_CHUNK: usize = 500;

pub struct RefreshDispatcharrLiveTvTask;

#[async_trait]
impl Task for RefreshDispatcharrLiveTvTask {
    fn key(&self) -> &str {
        "RefreshDispatcharrLiveTv"
    }
    fn name(&self) -> &str {
        "Refresh Dispatcharr Channel Versions & EPG"
    }
    fn description(&self) -> &str {
        "Attaches every Dispatcharr stream assigned to a channel as a selectable version of \
         that channel (in Dispatcharr's own priority order) and imports its programme guide, \
         both via Dispatcharr's authenticated API."
    }
    fn short_description(&self) -> &str {
        "Syncs per-channel stream versions and EPG from Dispatcharr"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::LiveTv
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let runtimes = ctx
            .addons
            .catalogs_for_kinds(&ctx, &[db::MediaKind::TvChannel])
            .await;

        let dispatcharr_runtimes: Vec<_> = runtimes
            .iter()
            .filter(|(runtime, _)| {
                runtime
                    .row
                    .preset
                    .kind
                    == "dispatcharr"
            })
            .collect();

        let client = crate::addons::dispatcharr::CLIENT.clone();

        for (idx, (runtime, _)) in dispatcharr_runtimes
            .iter()
            .enumerate()
        {
            progress.report(
                idx,
                dispatcharr_runtimes
                    .len()
                    .max(1),
            );

            let addon_id = runtime
                .row
                .id;
            let config = runtime
                .row
                .preset
                .config
                .expose();
            let base_url = config["base_url"]
                .as_str()
                .unwrap_or("")
                .trim_end_matches('/')
                .to_string();
            let token = config["api_key"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if base_url.is_empty() || token.is_empty() {
                continue;
            }

            let channels = match fetch_channels(&client, &base_url, &token).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr channels");
                    continue;
                }
            };

            debug!(addon = %addon_id, channels = channels.len(), "syncing Dispatcharr channel versions");

            let source_id = addon_id
                .simple()
                .to_string();
            let channel_media: Vec<db::Media> = channels
                .iter()
                .map(|ch| channel_to_media(ch, addon_id, &source_id))
                .collect();
            // Before the versions and recordings below, which reference these
            // rows by `parent_id`.
            if let Err(e) = db::Media::upsert(&ctx.db, &channel_media).await {
                error!(addon = %addon_id, error = %e, "failed to upsert Dispatcharr channels, skipping versions/EPG for this addon");
                continue;
            }

            // `epg_data_id -> tvg_id`: the assigned guide link, not the
            // channel's own `tvg_id` copy, which can go stale.
            let epg_data_map: HashMap<i64, String> = match fetch_epg_data(
                &client, &base_url, &token,
            )
            .await
            {
                Ok(rows) => rows
                    .into_iter()
                    .filter_map(|d| {
                        d.tvg_id
                            .map(|t| (d.id, t))
                    })
                    .collect(),
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr EPG data, EPG import will be skipped");
                    HashMap::new()
                }
            };

            // One value for the whole pass: `.streams()` filters on
            // `updated_at >= streams_refreshed_at`, so the children and the
            // parent marker must agree exactly or the writes are hidden.
            let now = chrono::Utc::now().naive_utc();
            let mut all_sources: Vec<db::Media> = Vec::new();
            let mut refreshed_channel_ids: Vec<uuid::Uuid> = Vec::new();
            // SD/HD variants can share one `tvg_id` and each needs its own
            // program rows, so this maps to every matching channel.
            let mut tvg_map: HashMap<String, Vec<uuid::Uuid>> = HashMap::new();
            let mut channel_ids: Vec<(&DispatcharrChannel, uuid::Uuid)> = Vec::new();

            for ch in &channels {
                let channel_media_id = uuid::Uuid::new_v5(
                    &addon_id,
                    format!("channel:{}", ch.id).as_bytes(),
                );
                if let Some(tvg_id) = ch
                    .epg_data_id
                    .and_then(|id| epg_data_map.get(&id))
                {
                    tvg_map
                        .entry(tvg_id.clone())
                        .or_default()
                        .push(channel_media_id);
                }

                channel_ids.push((ch, channel_media_id));
            }

            // One request per channel, so fan them out rather than walking
            // the list serially.
            let mut requests = Vec::with_capacity(channel_ids.len());
            for &(ch, channel_media_id) in &channel_ids {
                let client = &client;
                let (base_url, token) = (&base_url, &token);
                requests.push(async move {
                    match fetch_channel_streams(client, base_url, token, ch.id).await {
                        Ok(streams) => Some((ch, channel_media_id, streams)),
                        Err(e) => {
                            warn!(addon = %addon_id, channel = ch.id, error = %e, "failed to fetch channel streams");
                            None
                        }
                    }
                });
            }
            let fetched: Vec<_> = futures::stream::iter(requests)
                .buffer_unordered(CHANNEL_STREAM_CONCURRENCY)
                .filter_map(|r| async move { r })
                .collect()
                .await;

            for (ch, channel_media_id, streams) in fetched {
                if streams.is_empty() {
                    continue;
                }
                all_sources.extend(channel_versions(
                    ch,
                    &streams,
                    channel_media_id,
                    &base_url,
                    &token,
                    now,
                ));
                refreshed_channel_ids.push(channel_media_id);
            }

            if !all_sources.is_empty() {
                if let Err(e) = db::Media::upsert(&ctx.db, &all_sources).await {
                    error!(addon = %addon_id, error = %e, "failed to upsert Dispatcharr channel versions");
                } else {
                    // Two statements per chunk, not per channel. The delete's
                    // 1-day grace window is `AddonService::refresh_streams`'s,
                    // so a version dropped in Dispatcharr outlives any
                    // in-flight session on it.
                    for chunk in refreshed_channel_ids.chunks(SQL_BIND_CHUNK) {
                        let holes = vec!["?"; chunk.len()].join(",");
                        let update_sql = format!(
                            "UPDATE media SET streams_refreshed_at = ? WHERE id IN ({holes})"
                        );
                        let prune_sql = format!(
                            "DELETE FROM media WHERE kind = 'stream' \
                             AND parent_id IN ({holes}) \
                             AND updated_at < datetime('now', '-1 days')"
                        );
                        let mut update = sqlx::query(&update_sql).bind(now);
                        let mut prune = sqlx::query(&prune_sql);
                        for channel_id in chunk {
                            update = update.bind(channel_id);
                            prune = prune.bind(channel_id);
                        }
                        let _ = update
                            .execute(&ctx.db)
                            .await;
                        let _ = prune
                            .execute(&ctx.db)
                            .await;
                    }
                    info!(
                        addon = %addon_id,
                        channels = refreshed_channel_ids.len(),
                        versions = all_sources.len(),
                        "Dispatcharr channel versions synced"
                    );
                }
            }

            if tvg_map.is_empty() {
                continue;
            }

            let programs = match fetch_epg_grid(&client, &base_url, &token).await {
                Ok(p) => p,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr EPG grid");
                    continue;
                }
            };

            let import_start = chrono::Utc::now().naive_utc();
            let mut batch: Vec<db::Media> = Vec::with_capacity(500);
            let mut program_total = 0usize;
            for prog in &programs {
                let Some(channel_ids) = tvg_map.get(&prog.tvg_id) else {
                    continue;
                };
                let live_start = prog
                    .start_time
                    .naive_utc();
                let live_end = prog
                    .end_time
                    .naive_utc();
                for &channel_id in channel_ids {
                    let prog_id = uuid::Uuid::new_v5(
                        &channel_id,
                        format!("{}{}", prog.start_time, prog.title).as_bytes(),
                    );
                    batch.push(db::Media {
                        id: prog_id,
                        title: prog
                            .title
                            .clone(),
                        kind: db::MediaKind::TvProgram,
                        parent_id: Some(channel_id),
                        description: prog
                            .description
                            .clone()
                            .or_else(|| {
                                prog.sub_title
                                    .clone()
                            }),
                        live_start: Some(live_start),
                        live_end: Some(live_end),
                        ..Default::default()
                    });
                    program_total += 1;
                    if batch.len() >= 500 {
                        if let Err(e) = db::Media::upsert(&ctx.db, &batch).await {
                            warn!(addon = %addon_id, error = %e, "failed to upsert EPG batch");
                        }
                        batch.clear();
                    }
                }
            }
            if !batch.is_empty() {
                if let Err(e) = db::Media::upsert(&ctx.db, &batch).await {
                    warn!(addon = %addon_id, error = %e, "failed to upsert EPG batch");
                }
            }

            // Prune programs for these channels not re-imported this run,
            // and anything that ended over a day ago — same convention as
            // `iptv::stream_import_epg`.
            let channel_ids: Vec<uuid::Uuid> = tvg_map
                .values()
                .flatten()
                .copied()
                .collect();
            for chunk in channel_ids.chunks(200) {
                let mut qb = sqlx::QueryBuilder::new(
                    "DELETE FROM media WHERE kind = 'tv_program' AND updated_at < ",
                );
                qb.push_bind(import_start);
                qb.push(" AND parent_id IN (");
                let mut sep = qb.separated(", ");
                for id in chunk {
                    sep.push_bind(id);
                }
                qb.push(")");
                let _ = qb
                    .build()
                    .execute(&ctx.db)
                    .await;

                let mut qb2 = sqlx::QueryBuilder::new(
                    "DELETE FROM media WHERE kind = 'tv_program' \
                     AND live_end < datetime('now', '-1 day') AND parent_id IN (",
                );
                let mut sep2 = qb2.separated(", ");
                for id in chunk {
                    sep2.push_bind(id);
                }
                qb2.push(")");
                let _ = qb2
                    .build()
                    .execute(&ctx.db)
                    .await;
            }

            info!(addon = %addon_id, programs = program_total, "Dispatcharr EPG synced");
        }

        progress.set(100.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> sqlx::SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    #[test]
    fn task_identifies_itself_as_a_live_tv_task() {
        let task = RefreshDispatcharrLiveTvTask;
        assert_eq!(task.key(), "RefreshDispatcharrLiveTv");
        assert!(matches!(task.category(), TaskCategory::LiveTv));
        assert!(
            !task
                .name()
                .is_empty()
        );
        assert!(
            !task
                .description()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn migration_seeds_a_12h_trigger_for_this_task() {
        let db = test_db().await;
        let (task_id, kind, cron): (String, String, String) = sqlx::query_as(
            "SELECT task_id, kind, cron FROM task_triggers \
             WHERE id = 'default-dispatcharrlivetv-interval'",
        )
        .fetch_one(&db)
        .await
        .expect("default trigger row");

        assert_eq!(task_id, RefreshDispatcharrLiveTvTask.key());
        assert_eq!(kind, "IntervalTrigger");
        assert_eq!(cron, "0 0 */12 * * *");
        // Same 6-field form `TaskService` hands to the scheduler.
        tokio_cron_scheduler::Job::new(cron.as_str(), |_, _| {})
            .expect("cron expression must be valid");
    }

    #[tokio::test]
    async fn migration_is_the_only_default_trigger_for_this_task() {
        let db = test_db().await;
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_triggers WHERE task_id = 'RefreshDispatcharrLiveTv'",
        )
        .fetch_one(&db)
        .await
        .unwrap();
        assert_eq!(count, 1);
    }
}
