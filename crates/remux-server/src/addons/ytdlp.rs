use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use remux_sdks::stremio::MediaType;
use serde::Deserialize;
use sqlx::types::Json;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::process::Command;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, ResourceType,
};
use crate::db::{MetaResult, StreamProviderInfo};
use crate::{AppContext, api, db};

pub struct YtDlpPreset;

impl AddonPreset for YtDlpPreset {
    fn id(&self) -> &'static str {
        "ytdlp"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "ytdlp".to_string(),
            display_name: "yt-dlp".to_string(),
            description:
                "yt-dlp powered search, metadata, and stream resolution. Used \
                 for music tracks/albums via YouTube Music."
                    .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream],
            supported_types: vec![MediaType::Track, MediaType::Album],
            options: vec![AddonOption {
                id: "cookies".to_string(),
                name: "Cookies file".to_string(),
                description: Some(
                    "Path to a Netscape-format cookies file passed to yt-dlp via \
                     --cookies. Useful for age-restricted or login-required content."
                        .to_string(),
                ),
                required: false,
                default: None,
                kind: AddonOptionType::String,
            }],
        }
    }

    fn from_cfg(&self, cfg: &serde_json::Value) -> Result<Arc<dyn AddonKind>> {
        let cookies = cfg
            .get("cookies")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string);
        Ok(Arc::new(YtDlpAddon {
            cookies,
            executable: PathBuf::from("yt-dlp"),
        }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(YtDlpPreset))
}

pub struct YtDlpAddon {
    cookies: Option<String>,
    executable: PathBuf,
}

fn ytdlp_extra_args() -> Vec<String> {
    std::env::var("YTDLP_EXTRA_ARGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

impl YtDlpAddon {
    fn cookies_args(&self) -> Vec<String> {
        self.cookies
            .as_deref()
            .map(|path| vec!["--cookies".to_string(), path.to_string()])
            .unwrap_or_default()
    }
}

// ---------------------------------------------------------------------------
// Meta types
// ---------------------------------------------------------------------------

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
        if let Some(t) = self
            .thumbnails
            .iter()
            .filter(|t| !t.url.is_empty())
            .max_by_key(|t| (t.preference.unwrap_or(0), t.width.unwrap_or(0) as i64))
        {
            return Some(&t.url);
        }
        self.thumbnail.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Search types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Stream types
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// YtDlpAddon methods
// ---------------------------------------------------------------------------

impl YtDlpAddon {
    async fn dump_json(&self, url_or_query: &str) -> Result<YtDlpVideo> {
        let output = Command::new(&self.executable)
            .args([
                "--dump-json",
                "--no-playlist",
                "--quiet",
                "--no-warnings",
                url_or_query,
            ])
            .args(self.cookies_args())
            .args(ytdlp_extra_args())
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

    async fn resolve_watch_url(&self, media: &db::Media) -> Result<String> {
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

    async fn run_flat_playlist(
        &self,
        url_or_query: &str,
        limit: usize,
    ) -> Result<YtDlpPlaylist> {
        let limit_str = limit.to_string();
        let output = Command::new(&self.executable)
            .args([
                "--dump-single-json",
                "--flat-playlist",
                "--no-warnings",
                "--quiet",
                "--playlist-end",
                &limit_str,
                url_or_query,
            ])
            .args(self.cookies_args())
            .args(ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp exited with {}: {}", output.status, stderr.trim());
        }

        serde_json::from_slice(&output.stdout).with_context(|| {
            format!(
                "failed to parse yt-dlp flat-playlist JSON for '{}'",
                url_or_query
            )
        })
    }

    fn meta_can_refresh(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::Track && media.url.is_some()
    }

    async fn fetch_meta(&self, media: &db::Media) -> Result<Option<MetaResult>> {
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
            .ok_or_else(|| {
                anyhow::anyhow!("track has no URL or media_id for metadata fetch")
            })?;

        let output = Command::new(&self.executable)
            .args([
                "--dump-json",
                "--no-playlist",
                "--skip-download",
                "--quiet",
                "--no-warnings",
                &url,
            ])
            .args(self.cookies_args())
            .args(ytdlp_extra_args())
            .output()
            .await
            .context("failed to spawn yt-dlp for metadata")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp metadata failed for '{}': {}", url, stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| !l.trim().is_empty())
            .ok_or_else(|| {
                anyhow::anyhow!("yt-dlp produced no output for '{}'", url)
            })?;
        let meta: YtDlpMeta = serde_json::from_str(line).with_context(|| {
            format!("failed to parse yt-dlp metadata JSON for '{}'", url)
        })?;

        let poster = meta.best_thumbnail().map(|s| s.to_owned());
        Ok(Some(MetaResult {
            media: db::Media {
                title: if meta.title.is_empty() {
                    media.title.clone()
                } else {
                    meta.title
                },
                poster,
                description: meta.description,
                runtime: meta.duration.map(|d| d as i64),
                ..Default::default()
            },
            relations: vec![],
        }))
    }

    async fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<db::Media>> {
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
                let watch_url = entry.webpage_url.or(entry.url).or_else(|| {
                    if !entry.id.is_empty() {
                        Some(format!("https://www.youtube.com/watch?v={}", entry.id))
                    } else {
                        None
                    }
                });
                db::Media {
                    id: crate::common::get_stable_uuid(format!("ytdlp:{}", entry.id)),
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

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "yt-dlp track search done"
        );
        Ok(results)
    }

    async fn search_albums(&self, query: &str, limit: usize) -> Result<Vec<db::Media>> {
        tracing::debug!(query, limit, "yt-dlp album search starting");
        let t = std::time::Instant::now();

        let search_url = format!(
            "https://music.youtube.com/search?q={}&filter=albums",
            urlencoding::encode(query)
        );

        let output = Command::new(&self.executable)
            .args([
                "--dump-single-json",
                "--flat-playlist",
                "--no-warnings",
                "--quiet",
                &search_url,
            ])
            .args(self.cookies_args())
            .args(ytdlp_extra_args())
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

        tracing::debug!(
            query,
            count = album_urls.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "yt-dlp album stubs fetched"
        );

        if album_urls.is_empty() {
            return Ok(vec![]);
        }

        let exe = self.executable.clone();
        let cookies_args = self.cookies_args();
        let futures: Vec<_> = album_urls
            .into_iter()
            .map(|url| {
                let exe = exe.clone();
                let cookies_args = cookies_args.clone();
                async move {
                    let output = Command::new(&exe)
                        .args([
                            "--dump-single-json",
                            "--flat-playlist",
                            "--no-warnings",
                            "--quiet",
                            &url,
                        ])
                        .args(&cookies_args)
                        .args(ytdlp_extra_args())
                        .output()
                        .await
                        .ok()?;
                    if !output.status.success() {
                        return None;
                    }
                    let playlist: YtDlpPlaylist =
                        serde_json::from_slice(&output.stdout).ok()?;
                    Some((url, playlist))
                }
            })
            .collect();

        let albums = futures_util::future::join_all(futures).await;

        let results: Vec<_> = albums
            .into_iter()
            .flatten()
            .map(|(url, playlist)| {
                let thumbnail = playlist.thumbnail.or_else(|| {
                    playlist.entries.first().and_then(|e| e.thumbnail.clone())
                });
                db::Media {
                    id: crate::common::get_stable_uuid(format!(
                        "ytdlp-album:{}",
                        playlist.id
                    )),
                    title: playlist.title,
                    kind: db::MediaKind::Album,
                    media_id: Some(playlist.id.clone()),
                    url: Some(url),
                    poster: thumbnail,
                    ..Default::default()
                }
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t.elapsed().as_millis(),
            "yt-dlp album search done"
        );
        Ok(results)
    }

    async fn get_streams_for(&self, media: &db::Media) -> Result<Vec<db::Media>> {
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
                kind: db::MediaKind::Stream,
                title: f.label(),
                url: f.url.clone(),
                probe_data: Some(api::MediaSourceInfo {
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
                }),
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

// ---------------------------------------------------------------------------
// AddonKind impl
// ---------------------------------------------------------------------------

#[async_trait]
impl AddonKind for YtDlpAddon {
    fn id(&self) -> &'static str {
        "ytdlp"
    }

    async fn meta_supports(&self, media: &db::Media) -> bool {
        self.meta_can_refresh(media)
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        self.fetch_meta(media).await
    }

    async fn search_supports(&self, kind: &db::MediaKind) -> bool {
        matches!(kind, db::MediaKind::Track | db::MediaKind::Album)
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        match kind {
            db::MediaKind::Track => Ok(Some(self.search_tracks(query, limit).await?)),
            db::MediaKind::Album => Ok(Some(self.search_albums(query, limit).await?)),
            _ => Ok(None),
        }
    }

    async fn search_persist(
        &self,
        _id: Uuid,
        _ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        Ok(None)
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::Track
    }

    async fn stream_resolve(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        self.get_streams_for(media).await
    }
}
