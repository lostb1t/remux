use super::SubtitleProvider;
use crate::{db, sdks};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use sqlx::SqlitePool;

pub struct AioSubtitleProvider;

#[async_trait]
impl SubtitleProvider for AioSubtitleProvider {
    fn name(&self) -> &'static str {
        "aio"
    }

    fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
    }

    async fn fetch(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Result<Vec<sdks::aio::Subtitle>> {
        let aio = crate::aio::AioService::from_settings(db).await?;

        let (imdb_id, media_type, season, episode) = match media.kind {
            db::MediaKind::Movie => (
                media
                    .external_ids
                    .imdb
                    .as_deref()
                    .ok_or_else(|| anyhow!("no imdb_id"))?,
                sdks::aio::MediaType::Movie,
                None,
                None,
            ),
            db::MediaKind::Episode => (
                media
                    .series_media_id
                    .as_deref()
                    .ok_or_else(|| anyhow!("no series_media_id"))?,
                sdks::aio::MediaType::Series,
                media.parent_idx,
                media.idx,
            ),
            _ => return Err(anyhow!("subtitles not supported for {:?}", media.kind)),
        };

        aio.get_subtitles(media_type, imdb_id, season, episode)
            .await
    }
}
