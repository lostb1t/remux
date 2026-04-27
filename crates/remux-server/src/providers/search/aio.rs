use std::time::Duration;

use crate::{AppContext, aio, db, sdks};
use anyhow::Result;
use async_trait::async_trait;
use itertools::Itertools;
use uuid::Uuid;

use super::SearchService;

/// Search backend backed by AIO streams — handles movies and series.
///
/// Caches `sdks::aio::Meta` in `ctx.store` during search so that
/// [`persist`] can fetch full metadata and save to DB on first click.
pub struct AioSearchService;

#[async_trait]
impl SearchService for AioSearchService {
    fn supported_kinds(&self) -> &[db::MediaKind] {
        &[db::MediaKind::Movie, db::MediaKind::Series]
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let aio = aio::AioService::from_settings(&ctx.db).await?;

        let aio_type = match kind {
            db::MediaKind::Movie => sdks::aio::MediaType::Movie,
            db::MediaKind::Series => sdks::aio::MediaType::Series,
            _ => return Ok(vec![]),
        };

        let results = aio
            .search(aio_type, query.to_string())
            .await
            .unwrap_or_default();

        let mut media = results
            .into_iter()
            .unique_by(|m| {
                m.imdb_id
                    .as_ref()
                    .filter(|id| !id.is_empty())
                    .map(|id| format!("imdb:{}", id))
                    .unwrap_or_else(|| format!("{}:{}", m.media_type, m.id))
            })
            .take(limit)
            .filter_map(|meta| {
                let mut m = db::Media::try_from(meta.clone()).ok()?;
                let rels = crate::providers::meta::aio::build_relations(&m, &meta);
                m.relations = Some(
                    rels.into_iter()
                        .map(|r| (r.relation, r.media))
                        .collect(),
                );
                ctx.store
                    .insert(m.id.to_string(), meta, Duration::from_secs(3600));
                Some(m)
            })
            .collect();

        db::Media::enrich_parents(&ctx.db, &mut media).await;

        Ok(media)
    }

    async fn persist(&self, id: Uuid, ctx: &AppContext) -> Result<Option<db::Media>> {
        let meta = match ctx.store.get::<sdks::aio::Meta>(id.to_string()) {
            Some(m) => m,
            None => return Ok(None),
        };

        let aio = match aio::AioService::from_settings(&ctx.db).await {
            Ok(a) => a,
            Err(_) => return Ok(None),
        };

        let mut media: db::Media = aio
            .get_meta(meta.media_type.clone(), meta.id.clone())
            .await?
            .try_into()?;

        media.save(&ctx.db).await.ok();
        ctx.store.delete(id.to_string());

        // Re-fetch from DB to get the authoritative row (handles save conflicts).
        let saved = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                media_id: media.media_id.clone(),
                ..Default::default()
            },
        )
        .await?
        .records
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("media not found after save"))?;

        Ok(Some(saved))
    }
}
