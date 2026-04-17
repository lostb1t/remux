use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;

mod deezer;
mod ytdlp;
pub use deezer::DeezerMusicMetaProvider;
pub use ytdlp::YtDlpMusicMetaProvider;

/// Enriched metadata for a music item returned by a [`MusicMetaProvider`].
pub struct MusicMetaResult {
    pub media: db::Media,
}

/// Fetches and enriches metadata for music [`db::Media`] items.
///
/// Mirrors [`MetaProvider`] but scoped to music kinds (Track, Album, Artist).
/// Providers are chained in order — the primary fills first, subsequent
/// providers only fill `None` fields.
#[async_trait]
pub trait MusicMetaProvider: Send + Sync {
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MusicMetaResult>>;
}

/// Orchestrates music metadata enrichment across multiple providers.
pub struct MusicMetaProviderService {
    providers: Vec<Box<dyn MusicMetaProvider>>,
}

impl Default for MusicMetaProviderService {
    fn default() -> Self {
        Self {
            providers: vec![Box::new(DeezerMusicMetaProvider::default())],
        }
    }
}

impl MusicMetaProviderService {
    pub fn new(providers: Vec<Box<dyn MusicMetaProvider>>) -> Self {
        Self { providers }
    }

    /// Enrich metadata on a music item using registered providers in order.
    /// The primary provider (index 0) replaces when `force_refresh = true`;
    /// subsequent providers only fill `None` fields.
    pub async fn apply_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Result<()> {
        if !matches!(
            media.kind,
            db::MediaKind::Track | db::MediaKind::Album | db::MediaKind::Artist
        ) {
            return Ok(());
        }

        for (i, provider) in self.providers.iter().enumerate() {
            let replace = i == 0 && force_refresh;
            match provider.fetch(media, ctx).await {
                Ok(Some(result)) => merge_media(media, &result.media, replace),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(id = %media.id, error = %e, "music meta provider error");
                    continue;
                }
            }
        }
        Ok(())
    }
}

fn merge_media(target: &mut db::Media, source: &db::Media, replace: bool) {
    macro_rules! fill {
        ($field:ident) => {
            if replace || target.$field.is_none() {
                if source.$field.is_some() {
                    target.$field = source.$field.clone();
                }
            }
        };
    }
    if replace || target.title.is_empty() {
        if !source.title.is_empty() {
            target.title = source.title.clone();
        }
    }
    fill!(poster);
    fill!(description);
    fill!(released_at);
    fill!(runtime);
}
