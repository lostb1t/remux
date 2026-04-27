use crate::{AppContext, api, db};
use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use sqlx::types::Json;
use std::path::PathBuf;
use tokio::process::Command;

use super::StreamService;

/// Minimal subset of yt-dlp JSON output we actually need.
/// All fields are optional so missing/extra fields never cause parse failures.
#[derive(Debug, Deserialize)]
struct YtDlpVideo {
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    formats: Vec<YtDlpFormat>,
}

#[derive(Debug, Deserialize)]
struct YtDlpFormat {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    vcodec: Option<String>,
    #[serde(default)]
    acodec: Option<String>,
    #[serde(default)]
    tbr: Option<f64>,
    #[serde(default)]
    abr: Option<f64>,
    #[serde(default)]
    asr: Option<f64>,
    #[serde(default)]
    audio_channels: Option<i32>,
    #[serde(default)]
    format_note: Option<String>,
    #[serde(default)]
    format_id: Option<String>,
    #[serde(default)]
    ext: Option<String>,
    /// yt-dlp's own container field (e.g. "mp4", "webm") — more reliable than ext.
    #[serde(default)]
    container: Option<String>,
}

impl YtDlpFormat {
    fn is_audio_only(&self) -> bool {
        let no_video = self.vcodec.as_deref().map_or(false, |v| v == "none");
        let has_audio = self
            .acodec
            .as_deref()
            .map_or(false, |a| a != "none" && !a.is_empty());
        no_video && has_audio
    }

    fn bitrate(&self) -> Option<i64> {
        self.tbr.or(self.abr).map(|b| (b * 1000.0) as i64)
    }

    fn label(&self) -> String {
        self.format_note
            .clone()
            .or_else(|| self.format_id.clone())
            .unwrap_or_else(|| "audio".to_string())
    }

    fn container(&self) -> Option<String> {
        let raw = self.container.as_deref().or(self.ext.as_deref());
        match raw {
            Some("mp3") => Some("mp3".to_string()),
            Some("m4a") | Some("mp4") => Some("mp4".to_string()),
            Some("opus") | Some("webm") => Some("webm".to_string()),
            Some(other) => Some(other.to_string()),
            None => None,
        }
    }

    fn normalized_codec(&self) -> Option<String> {
        self.acodec
            .as_deref()
            .filter(|c| *c != "none")
            .map(|c| normalize_codec(c).to_string())
    }
}

fn normalize_codec(codec: &str) -> &str {
    if codec.starts_with("mp4a") {
        "aac"
    } else {
        codec
    }
}

/// Stream backend that shells out to the `yt-dlp` binary — handles music tracks.
pub struct YtDlpStreamService {
    executable: PathBuf,
}

impl Default for YtDlpStreamService {
    fn default() -> Self {
        Self {
            executable: PathBuf::from("yt-dlp"),
        }
    }
}

impl YtDlpStreamService {
    pub fn new(executable: PathBuf) -> Self {
        Self { executable }
    }

    /// Run `yt-dlp --dump-json <url_or_query>` and parse the first JSON object.
    async fn dump_json(&self, url_or_query: &str) -> Result<YtDlpVideo> {
        let output = Command::new(&self.executable)
            .args([
                "--dump-json",
                "--no-playlist",
                "--quiet",
                "--no-warnings",
                url_or_query,
            ])
            .args(crate::providers::ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp exited with {}: {}", output.status, stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .ok_or_else(|| {
                anyhow!("yt-dlp produced no output for '{}'", url_or_query)
            })?;

        serde_json::from_str(line).with_context(|| {
            format!("failed to parse yt-dlp JSON for '{}'", url_or_query)
        })
    }

    /// Resolve the stable YouTube watch URL for a track.
    ///
    /// Priority:
    /// 1. `media.url` — already a YouTube watch URL (cached from a previous lookup)
    /// 2. `media.media_id` that looks like a YouTube video ID (11 alphanumeric chars)
    /// 3. Search YouTube by `{title} {artist}` via `ytsearch1:`
    ///
    /// The watch URL is safe to persist in the DB; CDN stream URLs are not.
    pub async fn resolve_watch_url(&self, media: &db::Media) -> Result<String> {
        if let Some(url) = &media.url {
            return Ok(url.clone());
        }

        if let Some(id) = &media.media_id {
            if id.len() == 11
                && id
                    .chars()
                    .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
            {
                return Ok(format!("https://www.youtube.com/watch?v={}", id));
            }
        }

        let artist_part = media
            .description
            .as_deref()
            .and_then(|d| d.strip_prefix("by "))
            .unwrap_or("");
        let query = if artist_part.is_empty() {
            format!("ytsearch1:{}", media.title)
        } else {
            format!("ytsearch1:{} {}", media.title, artist_part)
        };

        tracing::debug!(?query, "searching YouTube for track");
        let video = self.dump_json(&query).await?;
        video
            .webpage_url
            .ok_or_else(|| anyhow!("yt-dlp search returned no webpage_url for query"))
    }
}

#[async_trait]
impl StreamService for YtDlpStreamService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let url = self.resolve_watch_url(media).await?;
        let video = self.dump_json(&url).await?;

        let to_source = |f: &YtDlpFormat| -> db::Media {
            let codec = f.normalized_codec();
            let display_title = match (codec.as_deref(), f.audio_channels) {
                (Some(c), Some(ch)) => format!("{} - {}ch", c.to_uppercase(), ch),
                (Some(c), None) => c.to_uppercase(),
                _ => f.label(),
            };
            db::Media {
                kind: db::MediaKind::Source,
                title: f.label(),
                url: f.url.clone(),
                probe_data: Some(Json(api::MediaSourceInfo {
                    container: f.container(),
                    run_time_ticks: media.runtime.map(|r| r * 10_000_000),
                    bitrate: f.bitrate(),
                    media_streams: vec![api::MediaStream {
                        index: 0,
                        type_: Some(api::MediaStreamType::Audio),
                        codec,
                        channels: f.audio_channels.map(|c| c as i64),
                        sample_rate: f.asr.map(|r| r as i64),
                        is_default: Some(true),
                        display_title: Some(display_title),
                        ..Default::default()
                    }],
                    ..Default::default()
                })),
                ..Default::default()
            }
        };

        let audio_only: Vec<db::Media> = video
            .formats
            .iter()
            .filter(|f| f.url.is_some() && f.is_audio_only())
            .map(to_source)
            .collect();

        if !audio_only.is_empty() {
            return Ok(audio_only);
        }

        Ok(video
            .formats
            .iter()
            .filter(|f| f.url.is_some())
            .map(to_source)
            .collect())
    }
}
