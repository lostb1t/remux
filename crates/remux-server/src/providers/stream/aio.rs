use crate::{AppContext, aio, db};
use anyhow::{Result, anyhow};
use async_trait::async_trait;

use super::{StreamOption, StreamService};

/// Stream backend backed by AIO — handles movies, series and episodes.
pub struct AioStreamService;

#[async_trait]
impl StreamService for AioStreamService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[
            db::MediaKind::Movie,
            db::MediaKind::Series,
            db::MediaKind::Episode,
        ]
    }

    async fn get_streams(&self, media: &db::Media, ctx: &AppContext) -> Result<Vec<StreamOption>> {
        let aio_svc = aio::AioService::from_settings(&ctx.db).await?;
        let media_type = db::media_kind_to_aio(&media.kind);
        let id = media
            .external_ids
            .imdb
            .clone()
            .or_else(|| media.media_id.clone())
            .ok_or_else(|| anyhow!("media has no identifiable ID for AIO stream lookup"))?;

        let streams = aio_svc.get_streams(media_type, id).await?;

        let options = streams
            .into_iter()
            .filter(|s| s.is_valid())
            .filter_map(|s| {
                let url = s.url.or(s.external_url)?;
                let label = s.name.unwrap_or_else(|| "AIO".to_string());
                Some(StreamOption {
                    url,
                    label,
                    mime_type: "video/mp4".to_string(),
                    is_audio_only: false,
                    ..Default::default()
                })
            })
            .collect();

        Ok(options)
    }
}
