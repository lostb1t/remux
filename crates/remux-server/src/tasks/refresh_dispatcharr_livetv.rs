use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use std::{collections::HashMap, sync::Arc};
use tracing::{debug, error, info, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{
    AppContext,
    addons::dispatcharr::{
        DispatcharrChannel, DispatcharrProgram, EpgWindow, channel_limit_of,
        channel_to_media, channel_versions, fetch_channel_streams, fetch_channels,
        fetch_epg_data, fetch_epg_grid, fetch_source_programs,
    },
    db,
};

/// Channel-stream requests in flight at once against one Dispatcharr.
const CHANNEL_STREAM_CONCURRENCY: usize = 8;

/// Channel ids per batched statement, under SQLite's bind-variable limit.
const SQL_BIND_CHUNK: usize = 500;

/// A Dispatcharr instance whose channel catalog is enabled.
fn syncable(
    runtime: &crate::addons::AddonRuntime,
    catalogs: &[crate::addons::ResolvedCatalog],
) -> bool {
    runtime
        .row
        .preset
        .kind
        == "dispatcharr"
        && catalogs
            .iter()
            .any(|c| c.enabled)
}

/// Stamps `channel_ids` as refreshed at `now`, which stops `Media::streams()`
/// serving versions this pass did not write, and deletes versions unwritten
/// for over a day. A failed delete is logged and left for a later pass.
async fn mark_streams_refreshed(
    db: &sqlx::SqlitePool,
    channel_ids: &[uuid::Uuid],
    now: chrono::NaiveDateTime,
) -> sqlx::Result<()> {
    for chunk in channel_ids.chunks(SQL_BIND_CHUNK) {
        let holes = vec!["?"; chunk.len()].join(",");
        let update_sql =
            format!("UPDATE media SET streams_refreshed_at = ? WHERE id IN ({holes})");
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
        update
            .execute(db)
            .await?;
        if let Err(e) = prune
            .execute(db)
            .await
        {
            warn!(error = %e, "failed to prune retired Dispatcharr channel versions");
        }
    }
    Ok(())
}

/// Prunes programmes for `channel_ids` that ended before `window_start` (or
/// over a day ago, if that is longer) and, when every write this pass
/// succeeded, those not re-imported this run.
async fn prune_programs(
    db: &sqlx::SqlitePool,
    channel_ids: &[uuid::Uuid],
    import_start: chrono::NaiveDateTime,
    all_written: bool,
    window_start: chrono::NaiveDateTime,
) {
    let oldest_kept = window_start.min(import_start - chrono::Duration::days(1));
    for chunk in channel_ids.chunks(200) {
        if all_written {
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
                .execute(db)
                .await;
        }

        let mut qb2 = sqlx::QueryBuilder::new(
            "DELETE FROM media WHERE kind = 'tv_program' AND live_end < ",
        );
        qb2.push_bind(oldest_kept);
        qb2.push(" AND parent_id IN (");
        let mut sep2 = qb2.separated(", ");
        for id in chunk {
            sep2.push_bind(id);
        }
        qb2.push(")");
        let _ = qb2
            .build()
            .execute(db)
            .await;
    }
}

/// Deletes every programme of `channel_ids`, channels with no guide assigned.
async fn clear_programs(db: &sqlx::SqlitePool, channel_ids: &[uuid::Uuid]) {
    for chunk in channel_ids.chunks(SQL_BIND_CHUNK) {
        let mut qb = sqlx::QueryBuilder::new(
            "DELETE FROM media WHERE kind = 'tv_program' AND parent_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in chunk {
            sep.push_bind(id);
        }
        qb.push(")");
        if let Err(e) = qb
            .build()
            .execute(db)
            .await
        {
            warn!(error = %e, "failed to clear programmes of channels without a guide");
        }
    }
}

/// Drops this addon's channel rows not written since `upsert_start`, i.e.
/// channels the fetch just completed no longer carries, along with their
/// versions, guide and recordings.
async fn prune_missing_channels(
    db: &sqlx::SqlitePool,
    source_id: &str,
    upsert_start: chrono::NaiveDateTime,
) {
    let result = sqlx::query(
        "DELETE FROM media WHERE kind = 'tv_channel' \
         AND json_extract(external_ids, '$.iptv_source_id') = ? \
         AND updated_at < ?",
    )
    .bind(source_id)
    .bind(upsert_start)
    .execute(db)
    .await;
    match result {
        Ok(r) if r.rows_affected() > 0 => {
            info!(
                source = %source_id,
                count = r.rows_affected(),
                "pruned Dispatcharr channels removed upstream"
            );
        }
        Ok(_) => {}
        Err(e) => {
            warn!(source = %source_id, error = %e, "failed to prune removed Dispatcharr channels")
        }
    }
}

/// `tvg_id -> source -> channels`, as the guide import builds it.
type GuideMap = HashMap<String, HashMap<Option<i64>, Vec<uuid::Uuid>>>;

/// Splits the guides into those the grid can place — it tags programmes with
/// `tvg_id` alone, so only an id used by a single source — and
/// `(tvg_id, source, channels)` for ids several sources share, which have to
/// be fetched one source at a time. The third part is the channels of such an
/// id that have no source to fetch by, and so get no programmes.
fn split_guides(
    guides: &GuideMap,
) -> (
    HashMap<&str, &[uuid::Uuid]>,
    Vec<(&str, i64, &[uuid::Uuid])>,
    Vec<uuid::Uuid>,
) {
    let mut from_grid = HashMap::new();
    let mut per_source = Vec::new();
    let mut unplaced = Vec::new();
    for (tvg_id, sources) in guides {
        if let [(_, channels)] = sources
            .iter()
            .collect::<Vec<_>>()
            .as_slice()
        {
            from_grid.insert(tvg_id.as_str(), channels.as_slice());
            continue;
        }
        for (source, channels) in sources {
            match *source {
                Some(source) => {
                    per_source.push((tvg_id.as_str(), source, channels.as_slice()))
                }
                None => unplaced.extend(channels),
            }
        }
    }
    (from_grid, per_source, unplaced)
}

/// Guide rows written per statement batch.
const PROGRAM_BATCH: usize = 500;

/// Writes and clears `rows`. `false` if the write failed.
async fn write_programs(
    db: &sqlx::SqlitePool,
    rows: &mut Vec<db::Media>,
    addon_id: uuid::Uuid,
) -> bool {
    let result = db::Media::upsert(db, rows).await;
    rows.clear();
    match result {
        Ok(()) => true,
        Err(e) => {
            warn!(addon = %addon_id, error = %e, "failed to upsert EPG batch");
            false
        }
    }
}

/// One guide row per channel carrying `prog`'s guide. A programme with no
/// title has nothing to show and is skipped.
fn program_rows(
    prog: &DispatcharrProgram,
    channel_ids: &[uuid::Uuid],
    source_id: &str,
) -> Vec<db::Media> {
    let Some(title) = prog
        .title
        .as_deref()
    else {
        return vec![];
    };
    channel_ids
        .iter()
        .map(|&channel_id| db::Media {
            id: crate::addons::dispatcharr::program_media_id(
                channel_id,
                prog.start_time,
                title,
            ),
            title: title.to_owned(),
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel_id),
            description: prog
                .description
                .clone()
                .or_else(|| {
                    prog.sub_title
                        .clone()
                }),
            live_start: Some(
                prog.start_time
                    .naive_utc(),
            ),
            live_end: Some(
                prog.end_time
                    .naive_utc(),
            ),
            external_ids: db::ExternalIds {
                iptv_source_id: Some(source_id.to_owned()),
                dispatcharr_program_id: prog
                    .id
                    .as_ref()
                    .and_then(crate::addons::dispatcharr::program_key),
                ..Default::default()
            },
            ..Default::default()
        })
        .collect()
}

/// Syncs Dispatcharr channel rows, their stream versions, their EPG and their
/// recordings.
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
            .filter(|(runtime, catalogs)| syncable(runtime, catalogs))
            .collect();

        let client = crate::addons::dispatcharr::CLIENT.clone();
        let global_max = crate::addons::dispatcharr::global_catalog_max(&ctx).await;

        for (idx, (runtime, catalogs)) in dispatcharr_runtimes
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

            let Some(limit) = channel_limit_of(catalogs, global_max) else {
                continue;
            };
            let mut channels = match fetch_channels(&client, &base_url, &token).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr channels");
                    continue;
                }
            };

            channels.truncate(limit);
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
            let upsert_start = chrono::Utc::now().naive_utc();
            if let Err(e) = db::Media::upsert(&ctx.db, &channel_media).await {
                error!(addon = %addon_id, error = %e, "failed to upsert Dispatcharr channels, skipping versions/EPG for this addon");
                continue;
            }
            prune_missing_channels(&ctx.db, &source_id, upsert_start).await;

            // `epg_data_id -> (tvg_id, source)`.
            let epg_data_map: Option<HashMap<i64, (String, Option<i64>)>> =
                match fetch_epg_data(&client, &base_url, &token).await {
                    Ok(rows) => Some(
                        rows.into_iter()
                            .filter_map(|d| {
                                d.tvg_id
                                    .map(|t| (d.id, (t, d.epg_source)))
                            })
                            .collect(),
                    ),
                    Err(e) => {
                        warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr EPG data, EPG import will be skipped");
                        None
                    }
                };

            // One value for the whole pass: `.streams()` filters on
            // `updated_at >= streams_refreshed_at`, so the children and the
            // parent marker must agree exactly or the writes are hidden.
            let now = chrono::Utc::now().naive_utc();
            let mut all_sources: Vec<db::Media> = Vec::new();
            let mut refreshed_channel_ids: Vec<uuid::Uuid> = Vec::new();
            // `tvg_id -> source -> channels`. SD/HD variants can share one
            // guide and each needs its own program rows, so a guide maps to
            // every matching channel; `tvg_id` is only unique per EPG source,
            // so the source is kept to tell two same-named guides apart.
            let mut tvg_map: GuideMap = HashMap::new();
            let mut channel_ids: Vec<(&DispatcharrChannel, uuid::Uuid)> = Vec::new();
            let mut unguided: Vec<uuid::Uuid> = Vec::new();

            for ch in &channels {
                let channel_media_id = uuid::Uuid::new_v5(
                    &addon_id,
                    format!("channel:{}", ch.id).as_bytes(),
                );
                if let Some((tvg_id, source)) = ch
                    .epg_data_id
                    .and_then(|id| {
                        epg_data_map
                            .as_ref()?
                            .get(&id)
                    })
                {
                    tvg_map
                        .entry(tvg_id.clone())
                        .or_default()
                        .entry(*source)
                        .or_default()
                        .push(channel_media_id);
                } else {
                    unguided.push(channel_media_id);
                }

                channel_ids.push((ch, channel_media_id));
            }

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

            // A channel is in `fetched` only if Dispatcharr answered for it,
            // so an empty list here is authoritative — the channel has no
            // assigned streams and its old versions must stop being served.
            // Only a failed request leaves a channel's versions alone.
            for (ch, channel_media_id, streams) in fetched {
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

            if !refreshed_channel_ids.is_empty() {
                let written = all_sources.is_empty()
                    || match db::Media::upsert(&ctx.db, &all_sources).await {
                        Ok(()) => true,
                        Err(e) => {
                            error!(addon = %addon_id, error = %e, "failed to upsert Dispatcharr channel versions");
                            false
                        }
                    };
                if written {
                    match mark_streams_refreshed(&ctx.db, &refreshed_channel_ids, now)
                        .await
                    {
                        Ok(()) => info!(
                            addon = %addon_id,
                            channels = refreshed_channel_ids.len(),
                            versions = all_sources.len(),
                            "Dispatcharr channel versions synced"
                        ),
                        Err(e) => {
                            error!(addon = %addon_id, error = %e, "failed to publish Dispatcharr channel versions")
                        }
                    }
                }
            }

            // Unconditional, unlike the programs below: a recording on a
            // channel with no guide data assigned must still sync.
            let dvr_cfg = crate::services::dvr_service::DvrConfig {
                addon_id,
                base_url: base_url.clone(),
                api_key: token.clone(),
            };
            match crate::services::DvrService::sync_recordings(&ctx, &dvr_cfg).await {
                Ok(count) => {
                    info!(addon = %addon_id, recordings = count, "Dispatcharr recordings synced");
                }
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to sync Dispatcharr recordings");
                }
            }

            if epg_data_map.is_some() {
                clear_programs(&ctx.db, &unguided).await;
            }

            if tvg_map.is_empty() {
                continue;
            }

            let (window_start, window_end) =
                EpgWindow::from_cfg(&config).bounds(chrono::Utc::now());
            let grid = match fetch_epg_grid(
                &client,
                &base_url,
                &token,
                window_start,
                window_end,
            )
            .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to fetch Dispatcharr EPG grid");
                    continue;
                }
            };

            let import_start = chrono::Utc::now().naive_utc();
            let mut all_written = true;
            let mut program_total = 0usize;
            let mut rows: Vec<db::Media> = Vec::with_capacity(PROGRAM_BATCH);

            let (from_grid, per_source, unplaced) = split_guides(&tvg_map);
            for prog in &grid {
                if let Some(channels) = prog
                    .tvg_id
                    .as_deref()
                    .and_then(|t| from_grid.get(t))
                {
                    rows.extend(program_rows(prog, channels, &source_id));
                    if rows.len() >= PROGRAM_BATCH {
                        program_total += rows.len();
                        all_written &=
                            write_programs(&ctx.db, &mut rows, addon_id).await;
                    }
                }
            }
            for (tvg_id, source, channels) in per_source {
                match fetch_source_programs(
                    &client,
                    &base_url,
                    &token,
                    tvg_id,
                    source,
                    window_start,
                    window_end,
                )
                .await
                {
                    Ok(programs) => {
                        rows.extend(
                            programs
                                .iter()
                                .flat_map(|p| program_rows(p, channels, &source_id)),
                        );
                        program_total += rows.len();
                        all_written &=
                            write_programs(&ctx.db, &mut rows, addon_id).await;
                    }
                    Err(e) => {
                        warn!(addon = %addon_id, tvg_id, source, error = %e, "failed to fetch Dispatcharr programmes for a shared guide id");
                        all_written = false;
                    }
                }
            }

            program_total += rows.len();
            all_written &= write_programs(&ctx.db, &mut rows, addon_id).await;

            let channel_ids: Vec<uuid::Uuid> = tvg_map
                .values()
                .flat_map(HashMap::values)
                .flatten()
                .copied()
                // Nothing was fetched for these, so their old programmes are
                // the last good ones.
                .filter(|id| !unplaced.contains(id))
                .collect();
            if !all_written {
                warn!(
                    addon = %addon_id,
                    "skipping stale-programme prune after a failed EPG write"
                );
            }
            prune_programs(
                &ctx.db,
                &channel_ids,
                import_start,
                all_written,
                window_start.naive_utc(),
            )
            .await;

            info!(addon = %addon_id, programs = program_total, "Dispatcharr EPG synced");
        }

        progress.set(100.0);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_guide_id_shared_by_two_sources_is_fetched_per_source() {
        let ch = |n: u128| uuid::Uuid::from_u128(n);
        let guides: GuideMap = HashMap::from([
            // SD/HD pair on one source: the grid places it.
            (
                "BBC1.uk".to_string(),
                HashMap::from([(Some(1), vec![ch(1), ch(2)])]),
            ),
            // Same id from two sources: the grid can't say which is which.
            (
                "ITV1.uk".to_string(),
                HashMap::from([(Some(1), vec![ch(3)]), (Some(2), vec![ch(4)])]),
            ),
            // Shared with a channel that has no source: nothing to fetch it by.
            (
                "NEWS".to_string(),
                HashMap::from([(Some(3), vec![ch(5)]), (None, vec![ch(6)])]),
            ),
        ]);
        let (from_grid, mut per_source, unplaced) = split_guides(&guides);
        assert_eq!(
            from_grid,
            HashMap::from([("BBC1.uk", [ch(1), ch(2)].as_slice())])
        );
        per_source.sort();
        assert_eq!(
            per_source,
            [
                ("ITV1.uk", 1, [ch(3)].as_slice()),
                ("ITV1.uk", 2, [ch(4)].as_slice()),
                ("NEWS", 3, [ch(5)].as_slice()),
            ]
        );
        assert_eq!(unplaced, [ch(6)]);
    }

    #[test]
    fn a_programme_without_a_title_makes_no_guide_rows() {
        let prog: DispatcharrProgram = serde_json::from_value(serde_json::json!({
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "title": null, "tvg_id": "BBC1.uk",
        }))
        .unwrap();
        assert!(program_rows(&prog, &[uuid::Uuid::from_u128(1)], "src").is_empty());
    }

    #[test]
    fn a_guide_row_keeps_dispatcharrs_programme_id_and_source() {
        let prog: DispatcharrProgram = serde_json::from_value(serde_json::json!({
            "id": 276710,
            "start_time": "2026-09-15T20:00:00Z",
            "end_time": "2026-09-15T21:00:00Z",
            "title": "News", "tvg_id": "BBC1.uk",
        }))
        .unwrap();
        let rows = program_rows(&prog, &[uuid::Uuid::from_u128(1)], "src");
        assert_eq!(
            rows[0]
                .external_ids
                .dispatcharr_program_id
                .as_deref(),
            Some("276710")
        );
        assert_eq!(
            rows[0]
                .external_ids
                .iptv_source_id
                .as_deref(),
            Some("src")
        );
    }

    async fn test_db() -> sqlx::SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    #[tokio::test]
    async fn the_migration_seeds_the_tasks_only_trigger_every_12h() {
        assert_eq!(
            RefreshDispatcharrLiveTvTask.key(),
            "RefreshDispatcharrLiveTv"
        );
        assert!(matches!(
            RefreshDispatcharrLiveTvTask.category(),
            TaskCategory::LiveTv
        ));
        let db = test_db().await;
        let triggers: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT id, kind, cron FROM task_triggers WHERE task_id = ?",
        )
        .bind(RefreshDispatcharrLiveTvTask.key())
        .fetch_all(&db)
        .await
        .unwrap();
        assert_eq!(
            triggers,
            [(
                "default-dispatcharrlivetv-interval".to_string(),
                "IntervalTrigger".to_string(),
                "0 0 */12 * * *".to_string()
            )]
        );
        // Same 6-field form `TaskService` hands to the scheduler.
        tokio_cron_scheduler::Job::new(
            triggers[0]
                .2
                .as_str(),
            |_, _| {},
        )
        .expect("cron expression must be valid");
    }

    fn runtime(kind: &str) -> crate::addons::AddonRuntime {
        let now = chrono::Utc::now().naive_utc();
        crate::addons::AddonRuntime {
            row: crate::addons::Addon {
                id: uuid::Uuid::from_u128(1),
                name: "test".into(),
                preset: crate::addons::AddonPresetRef {
                    kind: kind.into(),
                    config: serde_json::Value::Null.into(),
                },
                resources: vec![],
                types: vec![],
                enabled: true,
                priority: 0,
                created_at: now,
                updated_at: now,
                system: false,
                is_default: true,
                http_redirect_stream: false,
                service_filter: vec![],
            },
            caps: Default::default(),
        }
    }

    fn catalog(enabled: bool) -> crate::addons::ResolvedCatalog {
        crate::addons::ResolvedCatalog {
            provider_catalog_id: "channels".into(),
            catalog_id: "addon:1:channels".into(),
            collection_id: uuid::Uuid::from_u128(2),
            name: "Dispatcharr Channels".into(),
            media_kind: Some(db::MediaKind::TvChannel),
            collection_media_kind: None,
            enabled,
            max_items: None,
            tags: vec![],
        }
    }

    #[test]
    fn a_disabled_channel_catalog_is_not_synced() {
        assert!(syncable(&runtime("dispatcharr"), &[catalog(true)]));
        assert!(!syncable(&runtime("dispatcharr"), &[catalog(false)]));
        assert!(!syncable(&runtime("dispatcharr"), &[]));
        assert!(!syncable(&runtime("iptv-xtream"), &[catalog(true)]));
    }

    #[tokio::test]
    async fn a_channel_that_lost_every_stream_stops_serving_its_old_versions() {
        let db = test_db().await;
        let addon = uuid::Uuid::from_u128(0xd15b);
        let ch: crate::addons::dispatcharr::DispatcharrChannel =
            serde_json::from_value(serde_json::json!({
                "id": 42, "uuid": "u", "name": "BBC One"
            }))
            .unwrap();
        let mut channel = channel_to_media(&ch, addon, "src");
        let yesterday = chrono::Utc::now().naive_utc() - chrono::Duration::days(2);
        channel.streams_refreshed_at = Some(yesterday);
        db::Media::upsert(&db, &[channel.clone()])
            .await
            .unwrap();

        let version = db::Media {
            id: uuid::Uuid::new_v5(&channel.id, b"stream:1"),
            title: "1080p".into(),
            kind: db::MediaKind::Stream,
            parent_id: Some(channel.id),
            ..Default::default()
        };
        db::Media::upsert(&db, &[version])
            .await
            .unwrap();
        sqlx::query("UPDATE media SET streams_refreshed_at = ? WHERE id = ?")
            .bind(yesterday)
            .bind(channel.id)
            .execute(&db)
            .await
            .unwrap();

        let mut channel = db::Media::get_by_id(&db, &channel.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            channel
                .streams(&db)
                .await
                .unwrap()
                .len(),
            1,
            "the old version is being served before the refresh"
        );

        // The refresh fetched this channel and Dispatcharr returned no
        // streams for it, so nothing was upserted for it this pass.
        let now = chrono::Utc::now().naive_utc();
        mark_streams_refreshed(&db, &[channel.id], now)
            .await
            .unwrap();

        let mut channel = db::Media::get_by_id(&db, &channel.id)
            .await
            .unwrap()
            .unwrap();
        assert!(
            channel
                .streams(&db)
                .await
                .unwrap()
                .is_empty(),
            "a version Dispatcharr no longer assigns must not stay playable"
        );
    }

    /// A channel with one programme that ends tomorrow, stamped as imported
    /// before `import_start`.
    async fn seed_guide(db: &sqlx::SqlitePool) -> (uuid::Uuid, uuid::Uuid) {
        let addon = uuid::Uuid::from_u128(0xd15b);
        let ch: crate::addons::dispatcharr::DispatcharrChannel =
            serde_json::from_value(serde_json::json!({
                "id": 42, "uuid": "u", "name": "BBC One"
            }))
            .unwrap();
        let channel = channel_to_media(&ch, addon, "src");
        db::Media::upsert(db, &[channel.clone()])
            .await
            .unwrap();
        let program = db::Media {
            id: uuid::Uuid::new_v5(&channel.id, b"prog"),
            title: "Match of the Day".into(),
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel.id),
            live_start: Some(chrono::Utc::now().naive_utc()),
            live_end: Some(chrono::Utc::now().naive_utc() + chrono::Duration::days(1)),
            ..Default::default()
        };
        db::Media::upsert(db, &[program.clone()])
            .await
            .unwrap();
        (channel.id, program.id)
    }

    async fn program_count(db: &sqlx::SqlitePool) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM media WHERE kind = 'tv_program'")
            .fetch_one(db)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_failed_epg_write_does_not_take_the_last_good_guide_with_it() {
        let db = test_db().await;
        let (channel_id, _) = seed_guide(&db).await;
        // Every existing row predates this run, as they would when the batch
        // that should have refreshed them failed.
        let import_start = chrono::Utc::now().naive_utc() + chrono::Duration::hours(1);

        prune_programs(&db, &[channel_id], import_start, false, import_start).await;
        assert_eq!(
            program_count(&db).await,
            1,
            "a recoverable write failure must not clear the guide"
        );

        prune_programs(&db, &[channel_id], import_start, true, import_start).await;
        assert_eq!(
            program_count(&db).await,
            0,
            "after a clean run, a programme Dispatcharr dropped does go"
        );
    }

    #[tokio::test]
    async fn programmes_older_than_the_guide_window_go_even_after_a_failed_write() {
        let db = test_db().await;
        let (channel_id, program_id) = seed_guide(&db).await;
        sqlx::query("UPDATE media SET live_end = ? WHERE id = ?")
            .bind(chrono::Utc::now().naive_utc() - chrono::Duration::days(3))
            .bind(program_id)
            .execute(&db)
            .await
            .unwrap();

        let now = chrono::Utc::now().naive_utc();
        // A configured lookback of a week keeps a programme three days old.
        let week_ago = now - chrono::Duration::days(7);
        prune_programs(&db, &[channel_id], now, false, week_ago).await;
        assert_eq!(program_count(&db).await, 1);

        // A short lookback still drops what ended over a day ago.
        prune_programs(&db, &[channel_id], now, false, now).await;
        assert_eq!(program_count(&db).await, 0);
    }

    #[tokio::test]
    async fn a_channel_whose_guide_was_unassigned_loses_its_programmes() {
        let db = test_db().await;
        let (channel_id, _) = seed_guide(&db).await;

        clear_programs(&db, &[uuid::Uuid::from_u128(0xdead)]).await;
        assert_eq!(program_count(&db).await, 1, "other channels keep theirs");

        clear_programs(&db, &[channel_id]).await;
        assert_eq!(program_count(&db).await, 0);
    }

    #[tokio::test]
    async fn a_channel_removed_upstream_is_dropped_with_everything_under_it() {
        let db = test_db().await;
        let addon = uuid::Uuid::from_u128(0xd15b);
        let source = addon
            .simple()
            .to_string();
        let other = uuid::Uuid::from_u128(0x0dd);
        let make = |id: i64, addon: uuid::Uuid, source: &str| {
            let ch: crate::addons::dispatcharr::DispatcharrChannel =
                serde_json::from_value(serde_json::json!({
                    "id": id, "uuid": "u", "name": format!("Channel {id}")
                }))
                .unwrap();
            channel_to_media(&ch, addon, source)
        };

        let kept = make(42, addon, &source);
        let removed = make(43, addon, &source);
        let foreign = make(
            44,
            other,
            &other
                .simple()
                .to_string(),
        );
        db::Media::upsert(&db, &[kept.clone(), removed.clone(), foreign.clone()])
            .await
            .unwrap();
        let program = db::Media {
            id: uuid::Uuid::new_v5(&removed.id, b"prog"),
            title: "Gone".into(),
            kind: db::MediaKind::TvProgram,
            parent_id: Some(removed.id),
            ..Default::default()
        };
        db::Media::upsert(&db, &[program.clone()])
            .await
            .unwrap();

        // The next refresh returns only channel 42.
        let upsert_start = chrono::Utc::now().naive_utc();
        db::Media::upsert(&db, &[make(42, addon, &source)])
            .await
            .unwrap();
        prune_missing_channels(&db, &source, upsert_start).await;

        let present = |id: uuid::Uuid| {
            let db = db.clone();
            async move {
                db::Media::get_by_id(&db, &id)
                    .await
                    .unwrap()
                    .is_some()
            }
        };
        assert!(present(kept.id).await);
        assert!(!present(removed.id).await);
        assert!(!present(program.id).await, "its guide rows go with it");
        assert!(
            present(foreign.id).await,
            "another instance's channels are not this instance's business"
        );
    }
}
