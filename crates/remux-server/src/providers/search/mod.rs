use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use uuid::Uuid;

mod aio;
mod deezer;
mod musicbrainz;
mod tmdb_person;
mod ytdlp;
pub use aio::AioSearchService;
pub use deezer::{
    DeezerAlbumSearchService, DeezerArtistSearchService, DeezerTrackSearchService,
};
pub use musicbrainz::{MusicBrainzAlbumSearchService, MusicBrainzTrackSearchService};
pub use tmdb_person::TmdbPersonSearchService;
pub use ytdlp::{YtDlpAlbumSearchService, YtDlpSearchService};

/// Cached music search result — stored in `AppContext::store` keyed by `media.id`.
///
/// Allows the detail endpoint to persist a track/album and its related
/// album/artist records to the DB on first click, without needing a round-trip.
#[derive(Clone)]
pub struct MusicSearchResult {
    pub media: db::Media,
    /// The album this track belongs to (only set for Track results).
    pub album: Option<db::Media>,
    /// The primary artist (set for Track and Album results).
    pub artist: Option<db::Media>,
}

/// A pluggable search backend.
///
/// Each implementation declares which [`db::MediaKind`]s it handles via
/// [`supported_kinds`]. The search method is responsible for caching its results
/// so that [`persist`] can save them to the DB when the user opens the detail page.
#[async_trait]
pub trait SearchService: Send + Sync {
    fn supported_kinds(&self) -> &[db::MediaKind];

    /// Search and cache results. Returns `db::Media` for the API response.
    /// Implementations should store whatever is needed in `ctx.store` so
    /// that [`persist`] can later save the item to the database.
    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;

    /// Called when the user opens a detail page (`GET /Items/{id}`).
    ///
    /// If this provider owns the item (it has a cached entry for `id`), persist
    /// it (and any related records) to the DB and return the media.
    /// Return `Ok(None)` if this provider does not own the given ID.
    async fn persist(&self, id: Uuid, ctx: &AppContext) -> Result<Option<db::Media>> {
        let _ = (id, ctx);
        Ok(None)
    }
}

/// Routes search queries and persist calls to the appropriate [`SearchService`].
pub struct SearchServiceManager {
    services: Vec<Box<dyn SearchService>>,
}

impl Default for SearchServiceManager {
    fn default() -> Self {
        Self {
            services: vec![
                Box::new(AioSearchService),
                Box::new(DeezerTrackSearchService::default()),
                Box::new(DeezerAlbumSearchService::default()),
                Box::new(DeezerArtistSearchService::default()),
                Box::new(TmdbPersonSearchService),
            ],
        }
    }
}

impl SearchServiceManager {
    pub fn new(services: Vec<Box<dyn SearchService>>) -> Self {
        Self { services }
    }

    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        for svc in &self.services {
            if svc.supported_kinds().contains(kind) {
                tracing::debug!(
                    ?kind,
                    query,
                    limit,
                    "SearchServiceManager routing to service"
                );
                return svc.search(kind, query, limit, ctx).await;
            }
        }
        tracing::debug!(
            ?kind,
            query,
            "SearchServiceManager: no service registered for kind"
        );
        Ok(vec![])
    }

    /// Try each provider in order; return the first that claims ownership of `id`.
    pub async fn persist(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        for svc in &self.services {
            if let Some(media) = svc.persist(id, ctx).await? {
                return Ok(Some(media));
            }
        }
        Ok(None)
    }
}
