use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db, iptv};

pub struct IptvRefreshTask;

#[async_trait]
impl Task for IptvRefreshTask {
    fn key(&self) -> &str {
        "RefreshIptv"
    }
    fn name(&self) -> &str {
        "Refresh IPTV"
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
            info!("No IPTV sources configured, skipping refresh");
            return Ok(());
        }

        let client = reqwest::Client::new();
        let source_count = sources.len() as f64;
        let mut all_channels: Vec<db::Media> = Vec::new();

        for (idx, source) in sources.iter().enumerate() {
            progress.set((idx as f64 / source_count) * 80.0);
            info!(source = %source.name, kind = %source.source_type, "refreshing IPTV source");

            let xtream_epg_url = source.xtream_epg_url();
            let m3u_fetch_url = source.m3u_playlist_url().unwrap_or_default();

            let channels_parsed = if source.source_type == "xtream" {
                let user = source.xtream_username.as_deref().unwrap_or("");
                let pass = source.xtream_password.as_deref().unwrap_or("");
                debug!(source = %source.name, server = %source.m3u_url, "fetching Xtream live streams");
                match iptv::fetch_xtream_channels(&client, &source.m3u_url, user, pass).await {
                    Ok(ch) => {
                        info!(source = %source.name, count = ch.len(), "fetched Xtream channels");
                        ch
                    }
                    Err(e) => {
                        warn!(source = %source.name, error = %e, "failed to fetch Xtream channels");
                        continue;
                    }
                }
            } else {
                debug!(source = %source.name, url = %m3u_fetch_url, "fetching M3U");
                let resp = client.get(&m3u_fetch_url).send().await?;
                let status = resp.status();
                let m3u_text = resp.text().await?;
                debug!(
                    source = %source.name,
                    status = %status,
                    preview = %&m3u_text[..m3u_text.len().min(500)],
                    "M3U response"
                );
                let ch = iptv::parse_m3u(&m3u_text);
                info!(source = %source.name, count = ch.len(), "parsed M3U channels");
                ch
            };

            let source_uuid = source.id;

            let channels: Vec<db::Media> = channels_parsed
                .iter()
                .map(|ch| {
                    let tvg_key = ch.tvg_id.as_deref().unwrap_or(&ch.name);
                    let channel_id = Uuid::new_v5(&source_uuid, tvg_key.as_bytes());
                    db::Media {
                        id: channel_id,
                        title: ch.name.clone(),
                        kind: db::MediaKind::TvChannel,
                        url: Some(ch.url.clone()),
                        poster: ch.logo.clone(),
                        tvg_id: ch.tvg_id.clone(),
                        channel_number: ch.channel_number,
                        ..Default::default()
                    }
                })
                .collect();

            // Upsert first — conflict handler preserves user overrides (enabled,
            // sort_order, custom_name) for channels that still exist.
            db::Media::upsert(&ctx.db, &channels).await?;

            // For Xtream sources, fetch EPG from auto-derived URL
            if let Some(epg_url) = xtream_epg_url {
                match fetch_epg_programs(&client, &epg_url).await {
                    Ok(programs) => {
                        import_epg_programs(&ctx, &programs, &channels).await?;
                        info!(source = %source.name, programs = programs.len(), "imported Xtream EPG");
                    }
                    Err(e) => warn!(source = %source.name, error = %e, "failed to fetch Xtream EPG"),
                }
            }

            all_channels.extend(channels);
        }

        // Delete any tv_channel rows whose IDs are no longer produced by any source.
        // Doing this once after all sources are processed also handles deleted sources.
        if !all_channels.is_empty() {
            let kept: Vec<String> = all_channels.iter().map(|c| c.id.to_string()).collect();
            let placeholders = kept
                .iter()
                .enumerate()
                .map(|(i, _)| format!("${}", i + 1))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM media WHERE kind = 'tv_channel' AND id NOT IN ({placeholders})"
            );
            let mut q = sqlx::query(&sql);
            for id in &kept {
                q = q.bind(id);
            }
            q.execute(&ctx.db).await?;
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
                Err(e) => warn!(source = %epg_source.name, error = %e, "failed to fetch EPG"),
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

    // Delete old programs for channels we're about to update
    let channel_aio_ids: Vec<String> = channels
        .iter()
        .filter_map(|ch| ch.aio_id.clone())
        .collect::<std::collections::HashSet<_>>()
        .into_iter()
        .collect();

    for aio_id in &channel_aio_ids {
        sqlx::query(
            "DELETE FROM media WHERE kind = 'tv_program' AND parent_id IN (
                SELECT id FROM media WHERE aio_id = $1 AND kind = 'tv_channel'
            )",
        )
        .bind(aio_id)
        .execute(&ctx.db)
        .await?;
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
                ..Default::default()
            })
        })
        .collect();

    db::Media::upsert(&ctx.db, &media_programs).await?;
    Ok(())
}
