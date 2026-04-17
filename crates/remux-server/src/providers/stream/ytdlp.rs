use crate::{AppContext, db};
use anyhow::{Result, anyhow, Context};
use async_trait::async_trait;
use serde::Deserialize;
use sqlx::types::Json;
use std::path::PathBuf;
use tokio::process::Command;
use uuid::Uuid;

use crate::api;
use super::{StreamOption, StreamService};

/// Minimal subset of yt-dlp JSON output we actually need.
/// All fields are optional so missing/extra fields never cause parse failures.
#[derive(Debug, Deserialize)]
struct YtDlpVideo {
    #[serde(default)]
    webpage_url: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    duration: Option<f64>,
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
}

impl YtDlpFormat {
    fn is_audio_only(&self) -> bool {
        let no_video = self.vcodec.as_deref().map_or(false, |v| v == "none");
        let has_audio = self.acodec.as_deref().map_or(false, |a| a != "none" && !a.is_empty());
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
        match self.ext.as_deref() {
            Some("mp3") => Some("mp3".to_string()),
            Some("m4a") => Some("mp4".to_string()),
            Some("opus") | Some("webm") => Some("webm".to_string()),
            Some(other) => Some(other.to_string()),
            None => None,
        }
    }

    fn mime_type(&self) -> String {
        match self.ext.as_deref() {
            Some("mp3") => "audio/mpeg".to_string(),
            Some("m4a") => "audio/mp4".to_string(),
            Some("opus") | Some("webm") => "audio/webm".to_string(),
            _ => "audio/webm".to_string(),
        }
    }

    /// Build a `MediaSourceInfo` probe blob from this format's known fields.
    fn to_probe_data(&self, runtime: Option<i64>) -> api::MediaSourceInfo {
        let codec = self.acodec.clone().filter(|c| c != "none");
        let channels = self.audio_channels.map(|c| c as i64);
        let sample_rate = self.asr.map(|r| r as i64);
        let bitrate = self.bitrate();
        let container = self.container();

        let display_title = match (&codec, channels) {
            (Some(c), Some(ch)) => format!("{} - {}ch", c.to_uppercase(), ch),
            (Some(c), None) => c.to_uppercase(),
            (None, Some(ch)) => format!("Audio {}ch", ch),
            (None, None) => "Audio".to_string(),
        };

        api::MediaSourceInfo {
            container,
            run_time_ticks: runtime,
            bitrate,
            media_streams: vec![api::MediaStream {
                index: 0,
                type_: Some(api::MediaStreamType::Audio),
                codec,
                channels,
                sample_rate,
                is_default: Some(true),
                display_title: Some(display_title),
                ..Default::default()
            }],
            ..Default::default()
        }
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
                "--impersonate",
                "chrome",
                url_or_query,
            ])
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp exited with {}: {}", output.status, stderr.trim());
        }

        // yt-dlp may print multiple JSON lines for playlists; take the first.
        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .ok_or_else(|| anyhow!("yt-dlp produced no output for '{}'", url_or_query))?;

        serde_json::from_str(line)
            .with_context(|| format!("failed to parse yt-dlp JSON for '{}'", url_or_query))
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
        // 1. Use cached URL directly.
        if let Some(url) = &media.url {
            return Ok(url.clone());
        }

        // 2. If media_id looks like a real YouTube video ID (11 chars), use it.
        if let Some(id) = &media.media_id {
            if id.len() == 11 && id.chars().all(|c| c.is_alphanumeric() || c == '_' || c == '-') {
                return Ok(format!("https://www.youtube.com/watch?v={}", id));
            }
        }

        // 3. Search YouTube by title + artist name.
        // description stores "by {artist}" for Deezer tracks.
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

    /// Fetch format info for a track from yt-dlp and build a `db::Media` Source row
    /// with `probe_data` populated from the best audio-only format.
    ///
    /// The returned row has `kind = Source`, `parent_id = track.id`, `url = best stream URL`.
    /// Callers should upsert it to the DB so subsequent calls are cache hits.
    pub async fn get_audio_source_info(
        &self,
        track: &db::Media,
        watch_url: &str,
    ) -> Result<db::Media> {
        let video = self.dump_json(watch_url).await?;
        let runtime_ticks = track.runtime.map(|s| s * 10_000_000);

        // Pick best audio-only format (highest bitrate).
        let best = video
            .formats
            .iter()
            .filter(|f| f.url.is_some() && f.is_audio_only())
            .max_by(|a, b| {
                a.bitrate()
                    .unwrap_or(0)
                    .cmp(&b.bitrate().unwrap_or(0))
            });

        let (stream_url, probe_data) = match best {
            Some(f) => {
                tracing::debug!(
                    track_id = %track.id,
                    title = %track.title,
                    codec = ?f.acodec,
                    channels = ?f.audio_channels,
                    sample_rate = ?f.asr,
                    bitrate_kbps = ?f.abr.or(f.tbr),
                    ext = ?f.ext,
                    "audio source info resolved"
                );
                (f.url.clone().unwrap(), f.to_probe_data(runtime_ticks))
            }
            None => {
                // No audio-only formats — use first available format, no probe data.
                let fallback = video
                    .formats
                    .iter()
                    .find(|f| f.url.is_some())
                    .ok_or_else(|| anyhow!("yt-dlp returned no formats for {}", watch_url))?;
                (
                    fallback.url.clone().unwrap(),
                    fallback.to_probe_data(runtime_ticks),
                )
            }
        };

        // Stable deterministic ID: hash of track id so the same Source row is always
        // upserted rather than duplicated.
        let source_id = Uuid::new_v5(&track.id, b"audio_source");

        Ok(db::Media {
            id: source_id,
            kind: db::MediaKind::Source,
            title: track.title.clone(),
            url: Some(stream_url),
            parent_id: Some(track.id),
            runtime: track.runtime,
            probe_data: Some(Json(probe_data)),
            idx: Some(0),
            ..Default::default()
        })
    }
}

#[async_trait]
impl StreamService for YtDlpStreamService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Track]
    }

    async fn get_streams(&self, media: &db::Media, _ctx: &AppContext) -> Result<Vec<StreamOption>> {
        let url = self.resolve_watch_url(media).await?;
        let video = self.dump_json(&url).await?;

        let audio_only: Vec<StreamOption> = video
            .formats
            .iter()
            .filter(|f| f.url.is_some() && f.is_audio_only())
            .map(|f| StreamOption {
                url: f.url.clone().unwrap(),
                label: f.label(),
                mime_type: f.mime_type(),
                is_audio_only: true,
                bitrate: f.bitrate(),
            })
            .collect();

        if !audio_only.is_empty() {
            return Ok(audio_only);
        }

        // No audio-only formats — fall back to anything with a URL.
        Ok(video
            .formats
            .into_iter()
            .filter_map(|f| {
                let url = f.url.clone()?;
                Some(StreamOption {
                    url,
                    label: f.label(),
                    mime_type: f.mime_type(),
                    is_audio_only: false,
                    bitrate: f.bitrate(),
                })
            })
            .collect())
    }
}
