use crate::db;
use anyhow::Result;
use async_trait::async_trait;
use remux_sdks::remux::models::MediaSegments;
use sqlx::SqlitePool;

mod introdb;
mod probe;
pub use introdb::IntroDbSegmentProvider;
pub use probe::ProbeSegmentProvider;

#[async_trait]
pub trait SegmentProvider: Send + Sync {
    fn name(&self) -> &'static str;
    fn supports(&self, media: &db::Media) -> bool;
    async fn fetch(&self, media: &db::Media, db: &SqlitePool) -> Result<MediaSegments>;
}

/// Fetch segments for `media` from all matching providers and return the merged set.
/// Subsequent calls are cheap because providers use the global HTTP cache or read from DB.
pub async fn fetch(media: &db::Media, db: &SqlitePool) -> MediaSegments {
    let providers: &[&dyn SegmentProvider] =
        &[&ProbeSegmentProvider, &IntroDbSegmentProvider];
    let mut merged = MediaSegments::default();

    for p in providers {
        if !p.supports(media) {
            continue;
        }
        //  tracing::debug!(provider = p.name(), item = %media.id, "fetching segments");
        match p.fetch(media, db).await {
            Ok(segs) if !segs.is_empty() => {
                tracing::debug!(provider = p.name(), item = %media.id, seg_count = segs.to_pairs().len(), "segments fetched");
                merged.merge_from(segs);
            }
            Ok(_) => {}
            Err(e) => {
                tracing::error!(provider = p.name(), item = %media.id, error = %e, "segment provider failed");
            }
        }
    }
    tracing::info!(item = %media.id, seg_count = merged.to_pairs().len(), "segments fetched");
    merged
}
