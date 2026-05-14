use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use std::time::Instant;
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
        let mut all_channels: Vec<db::Media> = Vec::new();

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
                let m3u_text = resp.text().await?;
                iptv::parse_m3u(&m3u_text)
            };

            let source_uuid = source.id;
            let source_id = source.id.simple().to_string();

            let channels: Vec<db::Media> = channels_parsed
                .iter()
                .map(|ch| {
                    let tvg_key = ch.tvg_id.as_deref().unwrap_or(&ch.name);
                    let channel_id = Uuid::new_v5(&source_uuid, tvg_key.as_bytes());
                    db::Media {
                        id: channel_id,
                        title: ch.name.clone(),
                        kind: db::MediaKind::TvChannel,
                        stream_info: Some(crate::stream::StreamInfo {
                            descriptor: crate::stream::StreamDescriptor::http(
                                ch.url.clone(),
                            ),
                            ..Default::default()
                        }),
                        poster: ch.logo.clone(),
                        tvg_id: ch.tvg_id.clone(),
                        channel_number: ch.channel_number,
                        media_id: Some(source_id.clone()),
                        enabled: false,
                        program_kind: ch.program_kind.clone(),
                        ..Default::default()
                    }
                })
                .collect();

            // Upsert first — conflict handler preserves user overrides (enabled,
            // sort_order, custom_name) for channels that still exist.
            db::Media::upsert(&ctx.db, &channels).await?;

            // For Xtream sources, fetch EPG from auto-derived URL
            let mut epg_programs = 0usize;
            if let Some(epg_url) = xtream_epg_url {
                match fetch_epg_programs(&client, &epg_url).await {
                    Ok(programs) => {
                        epg_programs = programs.len();
                        import_epg_programs(&ctx, &programs, &channels).await?;
                    }
                    Err(e) => {
                        warn!(source = %source.name, error = %e, "failed to fetch Xtream EPG")
                    }
                }
            }

            info!(
                source = %source.name,
                kind = %source.source_type,
                channels = channels.len(),
                programs = epg_programs,
                elapsed_s = source_start.elapsed().as_secs(),
                "synced IPTV source"
            );

            all_channels.extend(channels);
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
                qb.push_values(chunk.iter(), |mut b, ch| {
                    b.push_bind(ch.id);
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
            match fetch_epg_programs(&client, &epg_source.url).await {
                Ok(programs) => {
                    import_epg_programs(&ctx, &programs, &all_channels).await?;
                    info!(source = %epg_source.name, programs = programs.len(), "imported EPG");
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

async fn fetch_epg_programs(
    client: &reqwest::Client,
    url: &str,
) -> Result<Vec<iptv::EpgProgram>> {
    let xml_text = client.get(url).send().await?.text().await?;
    iptv::parse_xmltv(&xml_text)
}

/// Match EPG programs to channels by tvg_id and upsert them.
async fn import_epg_programs(
    ctx: &AppContext,
    programs: &[iptv::EpgProgram],
    channels: &[db::Media],
) -> Result<()> {
    if programs.is_empty() || channels.is_empty() {
        return Ok(());
    }

    // Build tvg_id -> channel_id map
    let tvg_map: std::collections::HashMap<String, Uuid> = channels
        .iter()
        .filter_map(|ch| ch.tvg_id.as_ref().map(|t| (t.clone(), ch.id)))
        .collect();

    for chunk in channels.chunks(500) {
        let mut qb =
            sqlx::QueryBuilder::new("DELETE FROM media WHERE kind = 'tv_program'");
        qb.push(" AND parent_id IN (");
        let mut separated = qb.separated(", ");
        for channel in chunk {
            separated.push_bind(channel.id);
        }
        qb.push(")");
        qb.build().execute(&ctx.db).await?;
    }

    let media_programs: Vec<db::Media> = programs
        .iter()
        .filter_map(|prog| {
            let channel_id = tvg_map.get(&prog.channel_id)?;
            let prog_id = Uuid::new_v5(
                channel_id,
                format!(
                    "{}{}",
                    prog.start.map(|d| d.to_string()).unwrap_or_default(),
                    prog.title
                )
                .as_bytes(),
            );
            Some(db::Media {
                id: prog_id,
                title: prog.title.clone(),
                kind: db::MediaKind::TvProgram,
                parent_id: Some(*channel_id),
                description: prog.description.clone(),
                live_start: prog.start,
                live_end: prog.end,
                program_kind: prog.program_kind.clone(),
                poster: prog.poster.clone(),
                ..Default::default()
            })
        })
        .collect();

    db::Media::upsert(&ctx.db, &media_programs).await?;

    // Inherit program_kind from parent channel for programs that have none
    // (covers the case where EPG has no <category> tags but channels are categorised)
    sqlx::query(
        "UPDATE media SET program_kind = (
            SELECT c.program_kind FROM media c WHERE c.id = media.parent_id
        )
        WHERE kind = 'tv_program' AND program_kind IS NULL",
    )
    .execute(&ctx.db)
    .await?;

    Ok(())
}
