use crate::{AppContext, api, db};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use std::sync::Arc;

mod aio;
mod squid;
mod ytdlp;
pub use aio::AioStreamService;
pub use squid::SquidStreamService;
pub use ytdlp::YtDlpStreamService;

/// A resolved stream option returned by a [`StreamService`].
#[derive(Debug, Clone, Default)]
pub struct StreamOption {
    pub url: String,
    /// Human-readable label, e.g. "audio-only 128k" or "1080p".
    pub label: String,
    pub mime_type: String,
    pub is_audio_only: bool,
    /// Bitrate in bits-per-second, if known.
    pub bitrate: Option<i64>,
    // Probe fields — populated by services that already have this info (e.g. yt-dlp).
    pub codec: Option<String>,
    pub channels: Option<i64>,
    pub sample_rate: Option<i64>,
}

/// A pluggable stream-resolution backend.
///
/// Each implementation declares which [`db::MediaKind`]s it handles via
/// [`supported_kinds`]. [`StreamServiceManager`] routes to the first service
/// whose kinds include the media's kind.
#[async_trait]
pub trait StreamService: Send + Sync {
    fn supported_kinds(&self) -> &[db::MediaKind];
    async fn get_streams(&self, media: &db::Media, ctx: &AppContext) -> Result<Vec<StreamOption>>;
}

/// Routes stream resolution to the appropriate [`StreamService`] by media kind.
pub struct StreamServiceManager {
    services: Vec<Box<dyn StreamService>>,
    /// Direct reference to the yt-dlp service for URL pre-resolution.
    ytdlp: Option<Arc<YtDlpStreamService>>,
}

impl Default for StreamServiceManager {
    fn default() -> Self {
        let ytdlp = Arc::new(YtDlpStreamService::default());
        Self {
            services: vec![
                Box::new(AioStreamService),
                Box::new(SquidStreamService::default()),
                Box::new(YtDlpStreamService::default()),
            ],
            ytdlp: Some(ytdlp),
        }
    }
}

const STREAMS_TTL_SECS: i64 = 3600;

impl StreamServiceManager {
    pub fn new(services: Vec<Box<dyn StreamService>>) -> Self {
        Self { services, ytdlp: None }
    }

    /// Returns the yt-dlp service if one is registered.
    pub fn ytdlp(&self) -> Option<&YtDlpStreamService> {
        self.ytdlp.as_deref()
    }

    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<StreamOption>> {
        for svc in &self.services {
            if svc.supported_kinds().contains(&media.kind) {
                let streams = svc.get_streams(media, ctx).await?;
                if !streams.is_empty() {
                    return Ok(streams);
                }
            }
        }
        Ok(vec![])
    }

    /// Resolve streams for `media`, persist them as `Source` children in the DB,
    /// and stamp `streams_refreshed_at`. Skips if the stamp is < `STREAMS_TTL_SECS` old.
    pub async fn refresh_sources(&self, media: &db::Media, ctx: &AppContext) -> Result<()> {
        // Skip if recently refreshed.
        if let Some(refreshed) = media.streams_refreshed_at {
            let age = Utc::now().naive_utc() - refreshed;
            if age.num_seconds() < STREAMS_TTL_SECS {
                tracing::debug!(id = %media.id, age_secs = age.num_seconds(), "streams fresh, skipping refresh");
                return Ok(());
            }
        }

        let streams = self.get_streams(media, ctx).await?;
        if streams.is_empty() {
            return Ok(());
        }

        let sources: Vec<db::Media> = streams
            .into_iter()
            .enumerate()
            .map(|(idx, s)| stream_option_to_source(media, s, idx))
            .collect();

        db::Media::upsert(&ctx.db, &sources).await?;

        sqlx::query("UPDATE media SET streams_refreshed_at = CURRENT_TIMESTAMP WHERE id = ?")
            .bind(media.id)
            .execute(&ctx.db)
            .await?;

        // Remove Sources older than 7 days — they're too stale to be reached
        // by any ongoing playback session.
        sqlx::query(
            "DELETE FROM media WHERE kind = 'source' AND parent_id = ? AND updated_at < datetime('now', '-7 days')",
        )
        .bind(media.id)
        .execute(&ctx.db)
        .await?;

        tracing::debug!(id = %media.id, count = sources.len(), "streams refreshed");
        Ok(())
    }
}

fn stream_option_to_source(parent: &db::Media, s: StreamOption, idx: usize) -> db::Media {
    let runtime_ticks = parent.runtime.map(|r| r * 10_000_000);
    let display_title = match (&s.codec, s.channels) {
        (Some(c), Some(ch)) => format!("{} - {}ch", c.to_uppercase(), ch),
        (Some(c), None) => c.to_uppercase(),
        _ => s.label.clone(),
    };

    let probe_data = api::MediaSourceInfo {
        container: mime_to_container(&s.mime_type),
        run_time_ticks: runtime_ticks,
        bitrate: s.bitrate,
        media_streams: vec![api::MediaStream {
            index: 0,
            type_: Some(api::MediaStreamType::Audio),
            codec: s.codec,
            channels: s.channels,
            sample_rate: s.sample_rate,
            is_default: Some(true),
            display_title: Some(display_title),
            ..Default::default()
        }],
        ..Default::default()
    };

    // Stable deterministic ID so upsert always hits the same row.
    let source_id = uuid::Uuid::new_v5(&parent.id, format!("source_{idx}").as_bytes());

    let now = chrono::Utc::now().naive_utc();
    db::Media {
        id: source_id,
        kind: db::MediaKind::Source,
        title: s.label,
        url: Some(s.url),
        parent_id: Some(parent.id),
        runtime: parent.runtime,
        probe_data: Some(sqlx::types::Json(probe_data)),
        idx: Some(idx as i64),
        created_at: now,
        updated_at: now,
        ..Default::default()
    }
}

pub(super) fn normalize_codec(codec: &str) -> &str {
    if codec.starts_with("mp4a") { "aac" } else { codec }
}

fn mime_to_container(mime: &str) -> Option<String> {
    if mime.contains("flac") {
        Some("flac".to_string())
    } else if mime.contains("mp4") || mime.contains("m4a") {
        Some("mp4".to_string())
    } else if mime.contains("webm") || mime.contains("opus") {
        Some("webm".to_string())
    } else if mime.contains("mpeg") || mime.contains("mp3") {
        Some("mp3".to_string())
    } else {
        None
    }
}
