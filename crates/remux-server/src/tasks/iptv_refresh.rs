use anyhow::Result;
use async_trait::async_trait;
use futures::TryStreamExt;
use std::sync::Arc;
use std::time::Instant;
use tokio_util::io::{StreamReader, SyncIoBridge};
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db, db::IptvSourceType, iptv};

pub struct IptvRefreshTask;

#[async_trait]
impl Task for IptvRefreshTask {
    fn key(&self) -> &str {
        "RefreshIptv"
    }
    fn name(&self) -> &str {
        "Refresh IPTV"
    }
    fn description(&self) -> &str {
        "Reloads IPTV channel lists from all configured M3U sources."
    }
    fn short_description(&self) -> &str {
        "Reloads channel list from M3U sources"
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
        let sources = db::IptvSource::get_all(&ctx.db).await?;
        if sources.is_empty() {
            info!("No IPTV sources configured, removing IPTV channels");
            sqlx::query(
                "DELETE FROM media
                 WHERE kind = 'tv_program'
                   AND parent_id IN (SELECT id FROM media WHERE kind = 'tv_channel')",
            )
            .execute(&ctx.db)
            .await?;
            sqlx::query("DELETE FROM media WHERE kind = 'tv_channel'")
                .execute(&ctx.db)
                .await?;
            progress.set(100.0);
            return Ok(());
        }

        let client = reqwest::Client::new();
        let sources_progress = progress.scaled(0.0, 80.0);
        let mut all_channels: Vec<(Uuid, Option<String>)> = Vec::new();

        for (idx, source) in sources.iter().enumerate() {
            sources_progress.report(idx, sources.len());
            let source_start = Instant::now();

            let xtream_epg_url = source.xtream_epg_url();
            let m3u_fetch_url = source.m3u_playlist_url().unwrap_or_default();

            let channels_parsed = if source.source_type == IptvSourceType::Xtream {
                let user = source.xtream_username.as_deref().unwrap_or("");
                let pass = source.xtream_password.as_deref().unwrap_or("");
                let category_kinds =
                    iptv::fetch_xtream_categories(&client, &source.m3u_url, user, pass)
                        .await;
                debug!(source = %source.name, categories = category_kinds.len(), "fetched Xtream categories");
                match iptv::fetch_xtream_channels(
                    &client,
                    &source.m3u_url,
                    user,
                    pass,
                    &category_kinds,
                )
                .await
                {
                    Ok(ch) => ch,
                    Err(e) => {
                        warn!(source = %source.name, error = %e, "failed to fetch Xtream channels");
                        continue;
                    }
                }
            } else {
                debug!(source = %source.name, url = %m3u_fetch_url, "fetching M3U");
                let resp = client.get(&m3u_fetch_url).send().await?;
                iptv::parse_m3u_stream(resp).await?
            };

            let source_uuid = source.id;
            let source_id = source.id.simple().to_string();

            // Build and upsert db::Media in chunks of 1000 so at most 1000 full
            // structs exist at once, keeping peak RSS low for large Xtream sources.
            let channel_count = channels_parsed.len();
            let mut channel_refs: Vec<(Uuid, Option<String>)> =
                Vec::with_capacity(channel_count);

            for chunk in channels_parsed.chunks(1000) {
                let media_chunk: Vec<db::Media> = chunk
                    .iter()
                    .map(|ch| {
                        let tvg_key = ch.tvg_id.as_deref().unwrap_or(&ch.name);
                        let channel_id = Uuid::new_v5(&source_uuid, tvg_key.as_bytes());
                        let mut media = db::Media {
                            id: channel_id,
                            title: ch.name.clone(),
                            kind: db::MediaKind::TvChannel,
                            stream_info: Some(crate::stream::StreamInfo {
                                descriptor: crate::stream::StreamDescriptor::http(
                                    ch.url.clone(),
                                ),
                                ..Default::default()
                            }),
                            tvg_id: ch.tvg_id.clone(),
                            channel_number: ch.channel_number,
                            external_ids: db::ExternalIds {
                                iptv_source_id: Some(source_id.clone()),
                                iptv_group: ch.group.clone(),
                                ..Default::default()
                            },
                            enabled: false,
                            program_kind: ch.program_kind.clone(),
                            ..Default::default()
                        };
                        if let Some(url) = ch.logo.clone() {
                            media.set_image(db::ImageKind::Primary, url);
                        }
                        media
                    })
                    .collect();

                db::Media::upsert(&ctx.db, &media_chunk).await?;
                channel_refs
                    .extend(media_chunk.iter().map(|c| (c.id, c.tvg_id.clone())));
                // media_chunk dropped here — only 1000 structs ever in memory at once
            }
            drop(channels_parsed);

            // For Xtream sources, fetch EPG from auto-derived URL
            let mut epg_programs = 0usize;
            if let Some(epg_url) = xtream_epg_url {
                match stream_import_epg(&client, &epg_url, &channel_refs, &ctx).await {
                    Ok(count) => epg_programs = count,
                    Err(e) => {
                        warn!(source = %source.name, error = %e, "failed to fetch Xtream EPG")
                    }
                }
            }

            info!(
                source = %source.name,
                kind = %source.source_type,
                channels = channel_count,
                programs = epg_programs,
                elapsed_s = source_start.elapsed().as_secs(),
                "synced IPTV source"
            );

            all_channels.extend(channel_refs);
        }

        // Delete any tv_channel rows whose IDs are no longer produced by any source.
        // Uses a temp table to avoid SQLite's variable-count limit and stay fast.
        if !all_channels.is_empty() {
            let mut tx = ctx.db.begin().await?;
            sqlx::query(
                "CREATE TEMPORARY TABLE IF NOT EXISTS _iptv_kept (id BLOB NOT NULL PRIMARY KEY)",
            )
            .execute(&mut *tx)
            .await?;
            sqlx::query("DELETE FROM _iptv_kept")
                .execute(&mut *tx)
                .await?;

            for chunk in all_channels.chunks(500) {
                let mut qb =
                    sqlx::QueryBuilder::new("INSERT OR IGNORE INTO _iptv_kept (id) ");
                qb.push_values(chunk.iter(), |mut b, (id, _)| {
                    b.push_bind(*id);
                });
                qb.build().execute(&mut *tx).await?;
            }

            sqlx::query(
                "DELETE FROM media
                 WHERE kind = 'tv_program'
                   AND parent_id IN (
                       SELECT id FROM media
                       WHERE kind = 'tv_channel'
                         AND id NOT IN (SELECT id FROM _iptv_kept)
                   )",
            )
            .execute(&mut *tx)
            .await?;

            sqlx::query(
                "DELETE FROM media
                 WHERE kind = 'tv_channel'
                   AND id NOT IN (SELECT id FROM _iptv_kept)",
            )
            .execute(&mut *tx)
            .await?;
            tx.commit().await?;
        }

        // Fetch EPG from standalone epg_sources and match by tvg_id globally
        progress.set(85.0);
        let epg_sources = db::EpgSource::get_all(&ctx.db).await?;
        for epg_source in &epg_sources {
            debug!(source = %epg_source.name, url = %epg_source.url, "fetching EPG");
            match stream_import_epg(&client, &epg_source.url, &all_channels, &ctx).await
            {
                Ok(count) => {
                    info!(source = %epg_source.name, programs = count, "imported EPG")
                }
                Err(e) => {
                    warn!(source = %epg_source.name, error = %e, "failed to fetch EPG")
                }
            }
        }

        progress.set(100.0);
        Ok(())
    }
}

/// Stream-parse an XMLTV EPG, match each programme to a channel by tvg_id,
/// and upsert in batches of 2000. Never materialises Vec<EpgProgram> in full —
/// at most 2000 programs are in memory at once; only their UUIDs are kept for
/// the final stale-deletion step.
async fn stream_import_epg(
    client: &reqwest::Client,
    url: &str,
    channels: &[(Uuid, Option<String>)],
    ctx: &AppContext,
) -> Result<usize> {
    if channels.is_empty() {
        return Ok(0);
    }

    let tvg_map: std::collections::HashMap<String, Uuid> = channels
        .iter()
        .filter_map(|(id, tvg)| tvg.as_ref().map(|t| (t.clone(), *id)))
        .collect();

    let resp = client.get(url).send().await?;
    let byte_stream = resp
        .bytes_stream()
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e));
    let async_reader = StreamReader::new(byte_stream);
    let handle = tokio::runtime::Handle::current();

    // Channel between the blocking XML parser and the async DB importer.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<iptv::EpgProgram>(2000);

    let parse_handle = tokio::task::spawn_blocking(move || -> Result<()> {
        let sync_reader = SyncIoBridge::new_with_handle(async_reader, handle);
        let buf_reader = std::io::BufReader::with_capacity(256 * 1024, sync_reader);
        iptv::parse_xmltv(buf_reader, |prog| {
            tx.blocking_send(prog).ok();
        })
    });

    let mut batch: Vec<db::Media> = Vec::with_capacity(2000);
    // Collect only UUIDs (16 bytes each) for the stale-deletion step.
    let mut kept_ids: Vec<Uuid> = Vec::new();
    let mut total = 0usize;

    while let Some(prog) = rx.recv().await {
        let Some(&channel_id) = tvg_map.get(&prog.channel_id) else {
            continue;
        };
        let prog_id = Uuid::new_v5(
            &channel_id,
            format!(
                "{}{}",
                prog.start.map(|d| d.to_string()).unwrap_or_default(),
                prog.title
            )
            .as_bytes(),
        );
        let mut media = db::Media {
            id: prog_id,
            title: prog.title,
            kind: db::MediaKind::TvProgram,
            parent_id: Some(channel_id),
            description: prog.description,
            live_start: prog.start,
            live_end: prog.end,
            program_kind: prog.program_kind,
            ..Default::default()
        };
        if let Some(poster_url) = prog.poster {
            media.set_image(db::ImageKind::Primary, poster_url);
        }
        kept_ids.push(prog_id);
        batch.push(media);
        total += 1;

        if batch.len() >= 2000 {
            db::Media::upsert(&ctx.db, &batch).await?;
            batch.clear();
        }
    }

    if !batch.is_empty() {
        db::Media::upsert(&ctx.db, &batch).await?;
        drop(batch);
    }

    parse_handle.await??;

    // Stale deletion: everything in one transaction so the temp table is on a
    // single connection and stays visible across CREATE / INSERT / DELETE.
    {
        let mut tx = ctx.db.begin().await?;
        sqlx::query(
            "CREATE TEMPORARY TABLE IF NOT EXISTS _epg_kept (id BLOB NOT NULL PRIMARY KEY)",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM _epg_kept")
            .execute(&mut *tx)
            .await?;

        for chunk in kept_ids.chunks(500) {
            let mut qb =
                sqlx::QueryBuilder::new("INSERT OR IGNORE INTO _epg_kept (id) ");
            qb.push_values(chunk.iter(), |mut b, id| {
                b.push_bind(*id);
            });
            qb.build().execute(&mut *tx).await?;
        }

        for chunk in channels.chunks(500) {
            let mut qb = sqlx::QueryBuilder::new(
                "DELETE FROM media WHERE kind = 'tv_program' AND id NOT IN (SELECT id FROM _epg_kept) AND parent_id IN (",
            );
            let mut sep = qb.separated(", ");
            for (id, _) in chunk {
                sep.push_bind(*id);
            }
            qb.push(")");
            qb.build().execute(&mut *tx).await?;
        }

        tx.commit().await?;
    }

    // Inherit program_kind from parent channel for programs that have none.
    sqlx::query(
        "UPDATE media SET program_kind = (
            SELECT c.program_kind FROM media c WHERE c.id = media.parent_id
        )
        WHERE kind = 'tv_program' AND program_kind IS NULL",
    )
    .execute(&ctx.db)
    .await?;

    // Reap past programs (already aired) and return freed pages to the OS.
    sqlx::query("DELETE FROM media WHERE kind = 'tv_program' AND live_end < datetime('now', '-1 day')")
        .execute(&ctx.db)
        .await?;
    sqlx::query("PRAGMA incremental_vacuum(500)")
        .execute(&ctx.db)
        .await?;

    Ok(total)
}
