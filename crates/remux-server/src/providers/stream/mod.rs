use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

mod aio;
mod squid;
mod ytdlp;
pub use aio::AioStreamService;
pub use squid::SquidStreamService;
pub use ytdlp::YtDlpStreamService;

/// A resolved stream option returned by a [`StreamService`].
#[derive(Debug, Clone)]
pub struct StreamOption {
    pub url: String,
    /// Human-readable label, e.g. "audio-only 128k" or "1080p".
    pub label: String,
    pub mime_type: String,
    pub is_audio_only: bool,
    /// Bitrate in bits-per-second, if known.
    pub bitrate: Option<i64>,
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
}
