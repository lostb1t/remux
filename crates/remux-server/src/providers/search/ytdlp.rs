use crate::{AppContext, db};
use anyhow::{Result, Context};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;
use tokio::process::Command;

use super::SearchService;

/// Minimal yt-dlp playlist entry JSON fields.
#[derive(Debug, Deserialize)]
struct YtDlpEntry {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
}

/// Minimal yt-dlp playlist JSON fields.
#[derive(Debug, Deserialize)]
struct YtDlpPlaylist {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    entries: Vec<YtDlpEntry>,
}

/// Search backend backed by yt-dlp — handles music tracks.
pub struct YtDlpSearchService {
    executable: PathBuf,
}

impl Default for YtDlpSearchService {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("yt-dlp"),
        }
    }
}

impl YtDlpSearchService {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    async fn run_flat_playlist(&self, url_or_query: &str, limit: usize) -> Result<YtDlpPlaylist> {
        let limit_str = limit.to_string();
        let output = Command::new(&self.executable)
            .args(["--dump-single-json", "--flat-playlist", "--no-warnings", "--quiet", "--playlist-end", &limit_str, url_or_query])
            .args(super::ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp exited with {}: {}", output.status, stderr.trim());
        }

        serde_json::from_slice(&output.stdout)
            .with_context(|| format!("failed to parse yt-dlp flat-playlist JSON for '{}'", url_or_query))
    }
}

#[async_trait]
impl SearchService for YtDlpSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn search(&self, _kind: &db::MediaKind, query: &str, limit: usize, _ctx: &AppContext) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        let yt_query = format!("ytsearch{}:{}", limit, query);

        let playlist = match self.run_flat_playlist(&yt_query, limit).await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(query, error = %e, "yt-dlp search failed");
                return Err(e);
            }
        };

        let results: Vec<_> = playlist
            .entries
            .into_iter()
            .map(|entry| {
                let watch_url = entry.webpage_url
                    .or(entry.url)
                    .or_else(|| {
                        if !entry.id.is_empty() {
                            Some(format!("https://www.youtube.com/watch?v={}", entry.id))
                        } else {
                            None
                        }
                    });
                db::Media {
                    id: crate::utils::get_stable_uuid(format!("ytdlp:{}", entry.id)),
                    title: entry.title,
                    kind: db::MediaKind::Track,
                    media_id: Some(entry.id.clone()),
                    url: watch_url,
                    poster: entry.thumbnail,
                    runtime: entry.duration.map(|d| d as i64),
                    ..Default::default()
                }
            })
            .collect();

        tracing::info!(query, count = results.len(), elapsed_ms = t.elapsed().as_millis(), "yt-dlp track search done");

        Ok(results)
    }
}

/// Separate yt-dlp album search via YouTube Music.
pub struct YtDlpAlbumSearchService {
    executable: PathBuf,
}

impl Default for YtDlpAlbumSearchService {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("yt-dlp"),
        }
    }
}

#[async_trait]
impl SearchService for YtDlpAlbumSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Album]
    }

    async fn search(&self, _kind: &db::MediaKind, query: &str, limit: usize, _ctx: &AppContext) -> Result<Vec<db::Media>> {
        tracing::debug!(query, limit, "yt-dlp album search starting");
        let t = std::time::Instant::now();

        let search_url = format!(
            "https://music.youtube.com/search?q={}&filter=albums",
            urlencoding::encode(query)
        );

        // Step 1: get flat stub list — entries have IDs but no full metadata.
        let output = Command::new(&self.executable)
            .args(["--dump-single-json", "--flat-playlist", "--no-warnings", "--quiet", &search_url])
            .args(super::ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp for album search")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            tracing::warn!(query, error = %stderr.trim(), "yt-dlp album stub search failed");
            return Ok(vec![]);
        }

        let json: serde_json::Value = serde_json::from_slice(&output.stdout)?;
        let album_urls: Vec<String> = json["entries"]
            .as_array()
            .map(|entries| {
                entries
                    .iter()
                    .filter_map(|e| e["id"].as_str())
                    .filter(|id| id.starts_with("MPREb_"))
                    .take(limit.min(5))
                    .map(|id| format!("https://music.youtube.com/browse/{}", id))
                    .collect()
            })
            .unwrap_or_default();

        tracing::debug!(query, count = album_urls.len(), elapsed_ms = t.elapsed().as_millis(), "yt-dlp album stubs fetched");

        if album_urls.is_empty() {
            return Ok(vec![]);
        }

        // Step 2: fetch each album in parallel to get title + thumbnail.
        let exe = self.executable.clone();
        let futures: Vec<_> = album_urls
            .into_iter()
            .map(|url| {
                let exe = exe.clone();
                async move {
                    let output = Command::new(&exe)
                        .args(["--dump-single-json", "--flat-playlist", "--no-warnings", "--quiet", &url])
                        .args(super::super::ytdlp_extra_args())
                        .output()
                        .await
                        .ok()?;
                    if !output.status.success() {
                        return None;
                    }
                    let playlist: YtDlpPlaylist = serde_json::from_slice(&output.stdout).ok()?;
                    Some((url, playlist))
                }
            })
            .collect();

        let albums = futures_util::future::join_all(futures).await;

        let results: Vec<_> = albums
            .into_iter()
            .flatten()
            .map(|(url, playlist)| {
                let thumbnail = playlist.thumbnail
                    .or_else(|| playlist.entries.first().and_then(|e| e.thumbnail.clone()));
                db::Media {
                    id: crate::utils::get_stable_uuid(format!("ytdlp-album:{}", playlist.id)),
                    title: playlist.title,
                    kind: db::MediaKind::Album,
                    media_id: Some(playlist.id.clone()),
                    url: Some(url),
                    poster: thumbnail,
                    ..Default::default()
                }
            })
            .collect();

        tracing::info!(query, count = results.len(), elapsed_ms = t.elapsed().as_millis(), "yt-dlp album search done");

        Ok(results)
    }
}
