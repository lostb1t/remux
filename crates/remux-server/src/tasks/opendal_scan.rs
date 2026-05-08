use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures_util::TryStreamExt;
use opendal::EntryMode;
use regex::Regex;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::sdks::CachedEndpoint;
use crate::{AppContext, common, sdks};

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "ts", "wmv", "webm", "strm",
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "ogg", "opus", "wav", "aac", "wv", "strm",
];

pub struct OpendalScanTask;

#[async_trait]
impl Task for OpendalScanTask {
    fn key(&self) -> &str {
        "OpendalScan"
    }
    fn name(&self) -> &str {
        "Scan File Sources"
    }
    fn category(&self) -> &str {
        "Import"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let addons = crate::addons::addon::Addon::list(&ctx.db).await?;
        let opendal_addons: Vec<_> = addons
            .into_iter()
            .filter(|a| {
                a.enabled
                    && matches!(
                        a.preset.kind.as_str(),
                        "opendal-local" | "opendal-webdav"
                    )
            })
            .collect();

        if opendal_addons.is_empty() {
            progress.set(100.0);
            return Ok(());
        }

        let tmdb = common::tmdb_client(&ctx.db).await;
        let total = opendal_addons.len() as f64;

        for (idx, addon) in opendal_addons.iter().enumerate() {
            progress.set(idx as f64 / total * 90.0);
            if let Err(e) = scan_addon(&ctx, &tmdb, addon).await {
                warn!(addon = %addon.name, error = %e, "opendal scan failed");
            }
        }

        progress.set(100.0);
        Ok(())
    }
}

async fn scan_addon(
    ctx: &AppContext,
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    addon: &crate::addons::addon::Addon,
) -> Result<()> {
    let cfg = &addon.preset.config;
    let media_kind = cfg["media_kind"].as_str().unwrap_or("movie").to_string();

    let operator = build_operator(cfg, &addon.preset.kind)?;

    info!(addon = %addon.name, kind = %addon.preset.kind, media_kind, "opendal: scanning");

    let extensions: &[&str] = if media_kind == "track" {
        AUDIO_EXTENSIONS
    } else {
        VIDEO_EXTENSIONS
    };

    let ep_re =
        Regex::new(r"(?i)[Ss](\d{1,2})[Ee](\d{1,2})|(\d{1,2})[xX](\d{2})").unwrap();
    let year_re = Regex::new(r"\b((?:19|20)\d{2})\b").unwrap();
    let track_num_re = Regex::new(r"^(\d{1,3})[.\s\-_\[\]]+").unwrap();

    let mut lister = operator.lister_with("/").recursive(true).await?;
    let mut seen_ids: Vec<Uuid> = Vec::new();
    let mut upserted = 0usize;

    while let Some(entry) = lister.try_next().await? {
        if entry.metadata().mode() != EntryMode::FILE {
            continue;
        }

        let path = entry.path().to_string();
        let name = entry.name().to_string();
        let ext = std::path::Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if !extensions.contains(&ext.as_str()) {
            continue;
        }

        let row_id = common::get_stable_uuid(format!("{}:{}", addon.id, path));
        seen_ids.push(row_id);

        let stored_path: String = if ext == "strm" {
            match operator.read(&path).await {
                Ok(buf) => {
                    let url =
                        String::from_utf8_lossy(&buf.to_bytes()).trim().to_string();
                    if url.is_empty() {
                        warn!(path, "opendal: empty strm file, skipping");
                        continue;
                    }
                    url
                }
                Err(e) => {
                    warn!(path, error = %e, "opendal: failed to read strm, skipping");
                    continue;
                }
            }
        } else {
            path.clone()
        };

        let stem = std::path::Path::new(&name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&name)
            .to_string();

        let (title, season, episode, track_number, year, imdb_id) = match media_kind
            .as_str()
        {
            "track" => {
                let track_number = track_num_re
                    .captures(&stem)
                    .and_then(|c| c.get(1))
                    .and_then(|m| m.as_str().parse::<i64>().ok());
                let clean_stem = if track_number.is_some() {
                    track_num_re.replace(&stem, "").into_owned()
                } else {
                    stem.clone()
                };
                let title = clean_filename(&clean_stem, &ep_re, &year_re);
                (Some(title), None, None, track_number, None, None)
            }
            "episode" => {
                let (season, episode) = parse_episode(&stem, &ep_re);
                let year = parse_year(&stem, &year_re);
                let clean_title = clean_filename(&stem, &ep_re, &year_re);

                let existing_imdb = fetch_existing_imdb(ctx, addon.id, &path).await?;
                let imdb_id = if let Some(id) = existing_imdb {
                    Some(id)
                } else {
                    resolve_imdb(tmdb, &clean_title, None, true).await
                };

                if imdb_id.is_none() {
                    debug!(path, title = %clean_title, "opendal: no IMDB id, skipping");
                    continue;
                }

                (Some(clean_title), season, episode, None, year, imdb_id)
            }
            _ => {
                // movie
                let year = parse_year(&stem, &year_re);
                let clean_title = clean_filename(&stem, &ep_re, &year_re);

                let existing_imdb = fetch_existing_imdb(ctx, addon.id, &path).await?;
                let imdb_id = if let Some(id) = existing_imdb {
                    Some(id)
                } else {
                    resolve_imdb(tmdb, &clean_title, year, false).await
                };

                if imdb_id.is_none() {
                    debug!(path, title = %clean_title, "opendal: no IMDB id, skipping");
                    continue;
                }

                (Some(clean_title), None, None, None, year, imdb_id)
            }
        };

        let size = Some(entry.metadata().content_length() as i64);
        let now = Utc::now().naive_utc().to_string();

        sqlx::query(
            "INSERT INTO opendal_files \
             (id, addon_id, media_kind, path, name, title, imdb_id, season, episode, track_number, year, size, scanned_at) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
             ON CONFLICT(id) DO UPDATE SET \
               path = excluded.path, \
               name = excluded.name, media_kind = excluded.media_kind, \
               title = excluded.title, \
               imdb_id = COALESCE(opendal_files.imdb_id, excluded.imdb_id), \
               season = excluded.season, episode = excluded.episode, \
               track_number = excluded.track_number, \
               year = excluded.year, size = excluded.size, scanned_at = excluded.scanned_at",
        )
        .bind(row_id)
        .bind(addon.id)
        .bind(&media_kind)
        .bind(&stored_path)
        .bind(&name)
        .bind(title.as_deref())
        .bind(imdb_id.as_deref())
        .bind(season)
        .bind(episode)
        .bind(track_number)
        .bind(year)
        .bind(size)
        .bind(&now)
        .execute(&ctx.db)
        .await?;

        upserted += 1;
    }

    let deleted = prune_stale_paths(ctx, addon.id, &seen_ids).await?;

    info!(
        addon = %addon.name,
        upserted,
        deleted,
        "opendal: scan complete"
    );

    Ok(())
}

fn build_operator(
    cfg: &serde_json::Value,
    preset_kind: &str,
) -> Result<opendal::Operator> {
    match preset_kind {
        "opendal-webdav" => {
            let endpoint = cfg["endpoint"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("opendal-webdav: endpoint required"))?;
            let mut builder = opendal::services::Webdav::default().endpoint(endpoint);
            if let Some(u) = cfg["username"].as_str().filter(|s| !s.is_empty()) {
                builder = builder.username(u);
            }
            if let Some(p) = cfg["password"].as_str().filter(|s| !s.is_empty()) {
                builder = builder.password(p);
            }
            Ok(opendal::Operator::new(builder)?.finish())
        }
        "opendal-local" => {
            let path = cfg["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("opendal-local: path required"))?;
            Ok(
                opendal::Operator::new(opendal::services::Fs::default().root(path))?
                    .finish(),
            )
        }
        other => anyhow::bail!("opendal: unknown preset kind {:?}", other),
    }
}

fn parse_episode(stem: &str, ep_re: &Regex) -> (Option<i64>, Option<i64>) {
    if let Some(caps) = ep_re.captures(stem) {
        if let (Some(s), Some(e)) = (caps.get(1), caps.get(2)) {
            return (s.as_str().parse().ok(), e.as_str().parse().ok());
        }
        if let (Some(s), Some(e)) = (caps.get(3), caps.get(4)) {
            return (s.as_str().parse().ok(), e.as_str().parse().ok());
        }
    }
    (None, None)
}

fn parse_year(stem: &str, year_re: &Regex) -> Option<i64> {
    year_re
        .captures(stem)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

fn clean_filename(stem: &str, ep_re: &Regex, year_re: &Regex) -> String {
    let cut = ep_re
        .find(stem)
        .map(|m| m.start())
        .unwrap_or(usize::MAX)
        .min(year_re.find(stem).map(|m| m.start()).unwrap_or(usize::MAX));

    let raw = if cut == usize::MAX {
        stem.to_string()
    } else {
        stem[..cut].to_string()
    };

    raw.replace('.', " ")
        .replace('_', " ")
        .replace('-', " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim()
        .to_string()
}

async fn fetch_existing_imdb(
    ctx: &AppContext,
    addon_id: Uuid,
    path: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT imdb_id FROM opendal_files WHERE addon_id = ? AND path = ?",
    )
    .bind(addon_id)
    .bind(path)
    .fetch_optional(&ctx.db)
    .await?
    .flatten())
}

async fn resolve_imdb(
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    title: &str,
    year: Option<i64>,
    is_tv: bool,
) -> Option<String> {
    let client = tmdb.as_ref()?;
    if title.is_empty() {
        return None;
    }

    if is_tv {
        let resp = client
            .execute(
                sdks::tmdb::SearchTvEndpoint {
                    query: title.to_string(),
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;
        let tmdb_id = resp.results.into_iter().next()?.id;

        let series = client
            .execute(
                sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                    .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        series.external_ids.as_ref().and_then(|e| e.imdb_id.clone())
    } else {
        let resp = client
            .execute(
                sdks::tmdb::SearchMovieEndpoint {
                    query: title.to_string(),
                    year,
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;
        let tmdb_id = resp.results.into_iter().next()?.id;

        let movie = client
            .execute(
                sdks::tmdb::MovieEndpoint::new(tmdb_id)
                    .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        movie.imdb_id
    }
}

async fn prune_stale_paths(
    ctx: &AppContext,
    addon_id: Uuid,
    seen: &[Uuid],
) -> Result<usize> {
    if seen.is_empty() {
        let result = sqlx::query("DELETE FROM opendal_files WHERE addon_id = ?")
            .bind(addon_id)
            .execute(&ctx.db)
            .await?;
        return Ok(result.rows_affected() as usize);
    }

    let mut tx = ctx.db.begin().await?;
    sqlx::query(
        "CREATE TEMPORARY TABLE IF NOT EXISTS _opendal_seen (id BLOB NOT NULL PRIMARY KEY)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM _opendal_seen")
        .execute(&mut *tx)
        .await?;

    for chunk in seen.chunks(500) {
        let mut qb =
            sqlx::QueryBuilder::new("INSERT OR IGNORE INTO _opendal_seen (id) ");
        qb.push_values(chunk.iter(), |mut b, id| {
            b.push_bind(*id);
        });
        qb.build().execute(&mut *tx).await?;
    }

    let result = sqlx::query(
        "DELETE FROM opendal_files \
         WHERE addon_id = ? AND id NOT IN (SELECT id FROM _opendal_seen)",
    )
    .bind(addon_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(result.rows_affected() as usize)
}
