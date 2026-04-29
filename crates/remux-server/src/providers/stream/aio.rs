use crate::{AppContext, aio, db};
use anyhow::{Result, anyhow};
use async_trait::async_trait;
use tracing::instrument;

use super::{StreamProviderInfo, StreamService};

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

    #[instrument(skip(self, media, ctx), fields(media_id = %media.id, media_kind = ?media.kind, media_title = %media.title))]
    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let aio_svc = aio::AioService::from_settings(&ctx.db).await?;
        let media_type = db::media_kind_to_aio(&media.kind);
        let id = media
            .external_ids
            .imdb
            .clone()
            .or_else(|| media.media_id.clone())
            .ok_or_else(|| {
                anyhow!("media has no identifiable ID for AIO stream lookup")
            })?;

        let streams = aio_svc.get_streams(media_type, id).await?;

        Ok(streams
            .into_iter()
            .filter(|s| s.is_valid())
            .filter_map(|s| {
                let url = s.url.clone().or_else(|| s.external_url.clone())?;
                // Mirror `From<aio::Stream> for db::Media` — keep both the
                // addon name (provider/quality summary) and the description
                // (full codec/release breakdown). Clients render them with
                // the newline as a separator.
                let label = match (s.name.as_deref(), s.description.as_deref()) {
                    (Some(n), Some(d)) if !d.is_empty() => format!("{}\n{}", n, d),
                    (Some(n), _) => n.to_string(),
                    (None, Some(d)) => d.to_string(),
                    _ => "AIO".to_string(),
                };
                Some(db::Media {
                    kind: db::MediaKind::Source,
                    title: label,
                    url: Some(url),
                    provider_info: Some(sqlx::types::Json(StreamProviderInfo::Aio(s))),
                    ..Default::default()
                })
            })
            .collect())
    }
}
