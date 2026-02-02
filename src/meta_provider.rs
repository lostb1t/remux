use crate::{AppContext, aio, db};
use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{self, StreamExt};

#[async_trait]
pub trait MetaProvider: Send + Sync {
    async fn apply(&self, mut media: db::Media, ctx: AppContext) -> Result<db::Media>;

    async fn apply_many(
        &self,
        media: Vec<db::Media>,
        ctx: AppContext,
    ) -> Result<Vec<db::Media>> {
        use futures::stream::{self, StreamExt};

        let chunk_size = 10;
        let mut results = Vec::with_capacity(media.len());

        for chunk in media.chunks(chunk_size) {
            let ctx = ctx.clone();
            let this = self.clone();

            let futures = chunk.iter().cloned().map(move |m| {
                let ctx = ctx.clone();
                let this = this.clone();
                async move { this.apply(m, ctx).await }
            });

            let chunk_results = stream::iter(futures)
                .buffer_unordered(chunk_size)
                .collect::<Vec<_>>()
                .await;

            for res in chunk_results {
                results.push(res?);
            }
        }

        Ok(results)
    }
}

pub struct AioMetaProvider;

#[async_trait]
impl MetaProvider for AioMetaProvider {
    async fn apply(&self, media: db::Media, ctx: AppContext) -> Result<db::Media> {
        let meta = ctx
            .aio
            .get_meta(media.kind.clone().into(), media.aio_id.clone().unwrap())
            .await
            .context("Failed to fetch metadata")?;

        //  let media_new = meta.into();
        //media.title = metadata.title;
        //media.year = metadata.year;
        //media.genres = metadata.genres;
        Ok(media)
        // Ok(media_new<db::Media>[0])
    }
}
