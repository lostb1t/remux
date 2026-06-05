use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use serde::Deserialize;
use sqlx::types::Json;
use std::{path::PathBuf, sync::Arc};
use tokio::process::Command;
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, MediaKind, ResourceType,
};
use crate::{
    AppContext, api,
    common::{TickUnit, ToRunTimeTicks},
    db,
};

pub struct YtDlpPreset;

impl AddonPreset for YtDlpPreset {
    fn id(&self) -> &'static str {
        "ytdlp"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "ytdlp".to_string(),
            display_name: "yt-dlp".to_string(),
            description: "yt-dlp powered music stream resolution. Used \
                 for music via YouTube Music."
                .to_string(),
            icon: None,
            supported_resources: vec![ResourceType::Stream],
            supported_types: vec![MediaKind::Track],
            options: vec![
                AddonOption {
                    id: "cookies".to_string(),
                    name: "Cookies file".to_string(),
                    description: Some(
                        "Path to a Netscape-format cookies file passed to yt-dlp via \
                         --cookies."
                            .to_string(),
                    ),
                    required: false,
                    default: None,
                    kind: AddonOptionType::String,
                },
                AddonOption {
                    id: "cookies_content".to_string(),
                    name: "Cookies content".to_string(),
                    description: Some(
                        "Raw Netscape-format cookie text. Saved to the data directory \
                         and passed to yt-dlp via --cookies. Ignored when 'Cookies \
                         file' is set."
                            .to_string(),
                    ),
                    required: false,
                    default: None,
                    kind: AddonOptionType::String,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        config: &crate::Config,
    ) -> Result<Arc<dyn AddonKind>> {
        let cookies = cfg
            .get("cookies")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .or_else(|| {
                // Fallback for records saved before normalize_cfg was introduced.
                let content = cfg
                    .get("cookies_content")
                    .and_then(|v| v.as_str())
                    .filter(|s| !s.is_empty())?;
                let path = config
                    .data_dir
                    .join("yt-dlp-cookies.txt");
                std::fs::write(&path, content).ok()?;
                Some(
                    path.to_string_lossy()
                        .into_owned(),
                )
            });
        Ok(Arc::new(YtDlpAddon {
            cookies,
            executable: PathBuf::from("yt-dlp"),
            bgutil_script_path: config
                .bgutil_script_path
                .clone(),
        }))
    }

    fn normalize_cfg(
        &self,
        mut cfg: serde_json::Value,
        config: &crate::Config,
    ) -> Result<serde_json::Value> {
        let content = cfg
            .get("cookies_content")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .map(str::to_owned);

        if let Some(content) = content {
            let path = config
                .data_dir
                .join("yt-dlp-cookies.txt");
            std::fs::write(&path, content)
                .context("failed to write cookies content to data dir")?;
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert(
                    "cookies".to_string(),
                    serde_json::Value::String(
                        path.to_string_lossy()
                            .into_owned(),
                    ),
                );
                obj.remove("cookies_content");
            }
        }

        Ok(cfg)
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(YtDlpPreset))
}

pub struct YtDlpAddon {
    cookies: Option<String>,
    executable: PathBuf,
    bgutil_script_path: PathBuf,
}

fn ytdlp_extra_args() -> Vec<String> {
    std::env::var("YTDLP_EXTRA_ARGS")
        .unwrap_or_default()
        .split_whitespace()
        .map(str::to_owned)
        .collect()
}

impl YtDlpAddon {
    fn bgutil_args(&self) -> Vec<String> {
        if !self
            .bgutil_script_path
            .exists()
        {
            return vec![];
        }
        vec![
            "--extractor-args".to_string(),
            format!(
                "youtubepot-bgutilscript:script_path={}",
                self.bgutil_script_path
                    .display()
            ),
        ]
    }

    fn cookies_args(&self) -> Vec<String> {
        match &self.cookies {
            Some(path) => vec!["--cookies".to_string(), path.clone()],
            None => vec![],
        }
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
            .filter(|t| {
                !t.url
                    .is_empty()
            })
            .max_by_key(|t| {
                (
                    t.preference
                        .unwrap_or(0),
                    t.width
                        .unwrap_or(0) as i64,
                )
            })
        {
            return Some(&t.url);
        }
        self.thumbnail
            .as_deref()
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
        let no_video = self
            .vcodec
            .as_deref()
            .map_or(false, |v| v == "none");
        let has_audio = self
            .acodec
            .as_deref()
            .map_or(false, |a| a != "none" && !a.is_empty());
        no_video && has_audio
    }

    fn bitrate(&self) -> Option<i64> {
        self.tbr
            .or(self.abr)
            .map(|b| (b * 1000.0) as i64)
    }

    fn label(&self) -> String {
        self.format_note
            .clone()
            .or_else(|| {
                self.format_id
                    .clone()
            })
            .unwrap_or_else(|| "audio".to_string())
    }

    fn container(&self) -> Option<String> {
        let raw = self
            .container
            .as_deref()
            .or(self
                .ext
                .as_deref());
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
            .args(self.bgutil_args())
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output
            .status
            .success()
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp exited with {}: {}", output.status, stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| {
                !l.trim()
                    .is_empty()
            })
            .ok_or_else(|| {
                anyhow!("yt-dlp produced no output for '{}'", url_or_query)
            })?;

        serde_json::from_str(line).with_context(|| {
            format!("failed to parse yt-dlp JSON for '{}'", url_or_query)
        })
    }

    async fn resolve_watch_url(&self, media: &db::Media) -> Result<String> {
        if let Some(url) = media
            .stream_info
            .as_ref()
            .and_then(|si| {
                si.descriptor
                    .as_http_url()
            })
        {
            return Ok(url.to_owned());
        }
        if let Some(id) = &media
            .external_ids
            .youtube_id
        {
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
        let video = self
            .dump_json(&query)
            .await?;
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
            .args(self.bgutil_args())
            .output()
            .await
            .context("failed to spawn yt-dlp")?;

        if !output
            .status
            .success()
        {
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
        media.kind == db::MediaKind::Track
            && media
                .stream_info
                .is_some()
    }

    async fn fetch_meta(&self, media: &db::Media) -> Result<Option<db::Media>> {
        if media.kind != db::MediaKind::Track {
            return Ok(None);
        }
        let url = media
            .stream_info
            .as_ref()
            .and_then(|si| {
                si.descriptor
                    .as_http_url()
                    .map(str::to_owned)
            })
            .or_else(|| {
                media
                    .external_ids
                    .youtube_id
                    .as_deref()
                    .map(|id| format!("https://www.youtube.com/watch?v={}", id))
            })
            .ok_or_else(|| {
                anyhow::anyhow!("track has no URL or youtube_id for metadata fetch")
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
            .args(self.bgutil_args())
            .output()
            .await
            .context("failed to spawn yt-dlp for metadata")?;

        if !output
            .status
            .success()
        {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("yt-dlp metadata failed for '{}': {}", url, stderr.trim());
        }

        let stdout = String::from_utf8_lossy(&output.stdout);
        let line = stdout
            .lines()
            .find(|l| {
                !l.trim()
                    .is_empty()
            })
            .ok_or_else(|| {
                anyhow::anyhow!("yt-dlp produced no output for '{}'", url)
            })?;
        let meta: YtDlpMeta = serde_json::from_str(line).with_context(|| {
            format!("failed to parse yt-dlp metadata JSON for '{}'", url)
        })?;

        let thumbnail_url = meta
            .best_thumbnail()
            .map(|s| s.to_owned());
        let mut patch = db::Media {
            title: if meta
                .title
                .is_empty()
            {
                media
                    .title
                    .clone()
            } else {
                meta.title
            },
            description: meta.description,
            runtime: meta
                .duration
                .map(|d| d as i64),
            ..Default::default()
        };
        if let Some(url) = thumbnail_url {
            patch.set_image(db::ImageKind::Primary, url);
        }
        Ok(Some(patch))
    }

    async fn search_tracks(&self, query: &str, limit: usize) -> Result<Vec<db::Media>> {
        let t = std::time::Instant::now();
        let yt_query = format!("ytsearch{}:{}", limit, query);

        let playlist = match self
            .run_flat_playlist(&yt_query, limit)
            .await
        {
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
                let watch_url = entry
                    .webpage_url
                    .or(entry.url)
                    .or_else(|| {
                        if !entry
                            .id
                            .is_empty()
                        {
                            Some(format!(
                                "https://www.youtube.com/watch?v={}",
                                entry.id
                            ))
                        } else {
                            None
                        }
                    });
                let mut media = db::Media {
                    id: crate::common::stable_media_uuid(
                        &db::MediaKind::Track,
                        &entry.id,
                    ),
                    title: entry.title,
                    kind: db::MediaKind::Track,
                    stream_info: watch_url.map(|u| crate::stream::StreamInfo {
                        descriptor: crate::stream::StreamDescriptor::http(u),
                        ..Default::default()
                    }),
                    runtime: entry
                        .duration
                        .map(|d| d as i64),
                    external_ids: db::ExternalIds {
                        youtube_id: Some(
                            entry
                                .id
                                .clone(),
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                if let Some(url) = entry.thumbnail {
                    media.set_image(db::ImageKind::Primary, url);
                }
                media
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t
                .elapsed()
                .as_millis(),
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
            .args(self.bgutil_args())
            .output()
            .await
            .context("failed to spawn yt-dlp for album search")?;

        if !output
            .status
            .success()
        {
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
            elapsed_ms = t
                .elapsed()
                .as_millis(),
            "yt-dlp album stubs fetched"
        );

        if album_urls.is_empty() {
            return Ok(vec![]);
        }

        let exe = self
            .executable
            .clone();
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
                        .args(self.bgutil_args())
                        .output()
                        .await
                        .ok()?;
                    if !output
                        .status
                        .success()
                    {
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
                let thumbnail = playlist
                    .thumbnail
                    .or_else(|| {
                        playlist
                            .entries
                            .first()
                            .and_then(|e| {
                                e.thumbnail
                                    .clone()
                            })
                    });
                let mut media = db::Media {
                    id: crate::common::stable_media_uuid(
                        &db::MediaKind::Album,
                        &playlist.id,
                    ),
                    title: playlist.title,
                    kind: db::MediaKind::Album,
                    stream_info: Some(crate::stream::StreamInfo {
                        descriptor: crate::stream::StreamDescriptor::http(url),
                        ..Default::default()
                    }),
                    external_ids: db::ExternalIds {
                        youtube_id: Some(
                            playlist
                                .id
                                .clone(),
                        ),
                        ..Default::default()
                    },
                    ..Default::default()
                };
                if let Some(url) = thumbnail {
                    media.set_image(db::ImageKind::Primary, url);
                }
                media
            })
            .collect();

        tracing::debug!(
            query,
            count = results.len(),
            elapsed_ms = t
                .elapsed()
                .as_millis(),
            "yt-dlp album search done"
        );
        Ok(results)
    }

    async fn get_streams_for(
        &self,
        media: &db::Media,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let url = self
            .resolve_watch_url(media)
            .await?;
        let video = self
            .dump_json(&url)
            .await?;

        let to_source = |f: &YtDlpFormat| -> crate::stream::StreamInfo {
            let codec = f.normalized_codec();
            let display_title = match (codec.as_deref(), f.audio_channels) {
                (Some(c), Some(ch)) => format!("{} - {}ch", c.to_uppercase(), ch),
                (Some(c), None) => c.to_uppercase(),
                _ => f.label(),
            };
            crate::stream::StreamInfo {
                descriptor: crate::stream::StreamDescriptor::http(
                    f.url
                        .clone()
                        .unwrap_or_default(),
                ),
                name: Some(f.label()),
                probe_data: Some(api::MediaSourceInfo {
                    container: f.container(),
                    run_time_ticks: media
                        .runtime
                        .and_then(|r| r.to_ticks(TickUnit::Seconds)),
                    bitrate: f.bitrate(),
                    media_streams: vec![api::MediaStream {
                        index: 0,
                        type_: Some(api::MediaStreamType::Audio),
                        codec,
                        channels: f
                            .audio_channels
                            .map(|c| c as i64),
                        sample_rate: f
                            .asr
                            .map(|r| r as i64),
                        is_default: Some(true),
                        display_title: Some(display_title),
                        ..Default::default()
                    }],
                    ..Default::default()
                }),
                ..Default::default()
            }
        };

        let audio_only: Vec<crate::stream::StreamInfo> = video
            .formats
            .iter()
            .filter(|f| {
                f.url
                    .is_some()
                    && f.is_audio_only()
            })
            .map(&to_source)
            .collect();

        if !audio_only.is_empty() {
            return Ok(audio_only);
        }

        Ok(video
            .formats
            .iter()
            .filter(|f| {
                f.url
                    .is_some()
            })
            .map(&to_source)
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
        _config: &crate::api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        self.fetch_meta(media)
            .await
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
            db::MediaKind::Track => Ok(Some(
                self.search_tracks(query, limit)
                    .await?,
            )),
            db::MediaKind::Album => Ok(Some(
                self.search_albums(query, limit)
                    .await?,
            )),
            _ => Ok(None),
        }
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
        media.kind == db::MediaKind::Track
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        self.get_streams_for(media)
            .await
    }
}
