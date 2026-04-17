use crate::{AppContext, db};
use anyhow::{Result, Context};
use async_trait::async_trait;
use serde::Deserialize;
use std::path::PathBuf;
use tokio::process::Command;

use super::{MusicMetaProvider, MusicMetaResult};

/// Minimal yt-dlp video JSON fields needed for metadata enrichment.
#[derive(Debug, Deserialize)]
struct YtDlpMeta {
    #[serde(default)]
    title: String,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
    #[serde(default)]
    thumbnail: Option<String>,
    #[serde(default)]
    thumbnails: Vec<YtDlpThumbnail>,
}

#[derive(Debug, Deserialize)]
struct YtDlpThumbnail {
    #[serde(default)]
    url: String,
    #[serde(default)]
    preference: Option<i64>,
    #[serde(default)]
    width: Option<u64>,
}

impl YtDlpMeta {
    fn best_thumbnail(&self) -> Option<&str> {
        // Prefer the highest-preference thumbnail; fall back to widest, then first.
        if let Some(t) = self.thumbnails.iter()
            .filter(|t| !t.url.is_empty())
            .max_by_key(|t| (t.preference.unwrap_or(0), t.width.unwrap_or(0) as i64))
        {
            return Some(&t.url);
        }
        self.thumbnail.as_deref()
    }
}

/// Music metadata provider backed by yt-dlp — enriches tracks already identified by URL.
pub struct YtDlpMusicMetaProvider {
    executable: PathBuf,
}

impl Default for YtDlpMusicMetaProvider {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("yt-dlp"),
        }
    }
}

#[async_trait]
impl MusicMetaProvider for YtDlpMusicMetaProvider {
    async fn fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<MusicMetaResult>> {
        if media.kind != db::MediaKind::Track {
            return Ok(None);
        }

        let url = media
            .url
            .as_deref()
            .map(|u| u.to_owned())
            .or_else(|| {
                media
                    .media_id
                    .as_deref()
                    .map(|id| format!("https://www.youtube.com/watch?v={}", id))
            })
            .ok_or_else(|| anyhow::anyhow!("track has no URL or media_id for metadata fetch"))?;

        let output = Command::new(&self.executable)
            .args(["--dump-json", "--no-playlist", "--skip-download", "--quiet", "--no-warnings", &url])
            .args(super::super::ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp for metadata")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp metadata failed for '{}': {}", url, stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout.lines().find(|l| !l.trim().is_empty())
            .ok_or_else(|| anyhow::anyhow!("yt-dlp produced no output for '{}'", url))?;
        let meta: YtDlpMeta = serde_json::from_str(line)
            .with_context(|| format!("failed to parse yt-dlp metadata JSON for '{}'", url))?;

        let poster = meta.best_thumbnail().map(|s| s.to_owned());

        let enriched = db::Media {
            title: if meta.title.is_empty() { media.title.clone() } else { meta.title },
            poster,
            description: meta.description,
            runtime: meta.duration.map(|d| d as i64),
            ..Default::default()
        };

        Ok(Some(MusicMetaResult { media: enriched }))
    }
}
