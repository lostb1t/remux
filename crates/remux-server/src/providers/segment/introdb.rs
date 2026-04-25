use super::SegmentProvider;
use crate::db;
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use remux_sdks::remux::models::MediaSegments;
use sqlx::SqlitePool;

pub struct IntroDbSegmentProvider;

#[async_trait]
impl SegmentProvider for IntroDbSegmentProvider {
    fn name(&self) -> &'static str {
        "introdb"
    }

    fn supports(&self, media: &db::Media) -> bool {
        return matches!(media.kind, db::MediaKind::Episode | db::MediaKind::Source)
            && media.series_media_id.is_some()
            && media.parent_idx.is_some()
            && media.idx.is_some();
    }

    async fn fetch(
        &self,
        media: &db::Media,
        _db: &SqlitePool,
    ) -> Result<MediaSegments> {
        let imdb_id = media
            .series_media_id
            .as_deref()
            .ok_or_else(|| anyhow!("no series_media_id"))?;
        remux_sdks::introdb::fetch_episode_segments(
            imdb_id,
            media.parent_idx.unwrap(),
            media.idx.unwrap(),
        )
        .await
    }
}
