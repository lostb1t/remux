use crate::{AppContext, db};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};

mod aio;
mod squid;
mod ytdlp;
pub use aio::AioStreamService;
pub use squid::SquidStreamService;
pub use ytdlp::YtDlpStreamService;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProviderInfo {
    Aio(crate::sdks::aio::Stream),
}

/// A pluggable stream-resolution backend.
///
/// Each implementation declares which [`db::MediaKind`]s it handles via
/// [`supported_kinds`]. [`StreamServiceManager`] routes to the first service
/// whose kinds include the media's kind.
#[async_trait]
pub trait StreamService: Send + Sync {
    fn supported_kinds(&self) -> &[db::MediaKind];
    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;
}

/// Routes stream resolution to the appropriate [`StreamService`] by media kind.
pub struct StreamServiceManager {
    services: Vec<Box<dyn StreamService>>,
}

impl Default for StreamServiceManager {
    fn default() -> Self {
        Self {
            services: vec![
                Box::new(AioStreamService),
                Box::new(SquidStreamService::default()),
                Box::new(YtDlpStreamService::default()),
            ],
        }
    }
}

const STREAMS_TTL_SECS: i64 = 3600;

impl StreamServiceManager {
    pub fn new(services: Vec<Box<dyn StreamService>>) -> Self {
        Self { services }
    }

    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        for svc in &self.services {
            if svc.supported_kinds().contains(&media.kind) {
                match svc.get_streams(media, ctx).await {
                    Ok(streams) if !streams.is_empty() => return Ok(streams),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(error = %e, "stream service failed"),
                }
            }
        }
        Ok(vec![])
    }

    /// Resolve streams for `media`, persist them as `Source` children in the DB,
    /// and stamp `streams_refreshed_at`. Skips if the stamp is < `STREAMS_TTL_SECS` old.
    pub async fn refresh_sources(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<()> {
        if let Some(refreshed) = media.streams_refreshed_at {
            let age = Utc::now().naive_utc() - refreshed;
            if age.num_seconds() < STREAMS_TTL_SECS {
                tracing::debug!(id = %media.id, age_secs = age.num_seconds(), "streams fresh, skipping refresh");
                return Ok(());
            }
        }

        let raw = self.get_streams(media, ctx).await?;
        if raw.is_empty() {
            return Ok(());
        }

        let now = Utc::now().naive_utc();
        let sources: Vec<db::Media> = raw
            .into_iter()
            .enumerate()
            .map(|(idx, mut s)| {
                s.id =
                    uuid::Uuid::new_v5(&media.id, format!("source_{idx}").as_bytes());
                s.parent_id = Some(media.id);
                s.runtime = media.runtime;
                s.idx = Some(idx as i64);
                s.created_at = now;
                s.updated_at = now;
                s
            })
            .collect();

        db::Media::upsert(&ctx.db, &sources).await?;

        sqlx::query(
            "UPDATE media SET streams_refreshed_at = CURRENT_TIMESTAMP WHERE id = ?",
        )
        .bind(media.id)
        .execute(&ctx.db)
        .await?;

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
