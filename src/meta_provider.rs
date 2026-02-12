use crate::{AppContext, aio, db, sdks};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use tracing::error;

pub struct MetaProviderService;

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

            for m in chunk {
                let ctx = ctx.clone();
                let this = self.clone();
                let media_title = m.title.clone();

                match this.refresh_tree(m.clone(), ctx).await {
                    Ok(media_vec) => results.extend(media_vec),
                    Err(e) => {
                        error!("Failed to process media '{}': {}", media_title, e)
                    }
                }
            }
        }

        Ok(results)
    }

    async fn refresh_tree(
        &self,
        media: db::Media,
        ctx: AppContext,
    ) -> Result<Vec<db::Media>> {
        let mut all_media = Vec::new();
        let media = self.apply(media, ctx.clone()).await?;
        all_media.push(media.clone());

        if media.kind == db::MediaKind::Series {
            if let Some(seasons) = self.get_seasons(media.clone(), ctx.clone()).await? {
                let seasons = self.apply_many(seasons, ctx.clone()).await?;
                all_media.extend(seasons.clone());
                for season in seasons {
                    if let Some(episodes) =
                        self.get_episodes(season.clone(), ctx.clone()).await?
                    {
                        let episodes = self.apply_many(episodes, ctx.clone()).await?;
                        all_media.extend(episodes);
                    }
                }
            }
        }

        Ok(all_media)
    }

    async fn get_seasons(
        &self,
        mut media: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>>;

    async fn get_episodes(
        &self,
        mut media: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
}

pub struct AioMetaProvider;

#[async_trait]
impl MetaProvider for AioMetaProvider {
    async fn apply(&self, mut media: db::Media, ctx: AppContext) -> Result<db::Media> {
        let meta = ctx
            .aio
            .get_meta(
                media.kind.clone().into(),
                media
                    .series_imdb_id
                    .clone()
                    .or(media.imdb_id.clone())
                    .unwrap(),
            )
            .await?;
        // .context("Failed to fetch metadata")?;

        let media_new: db::Media = meta.try_into()?;
        media.title = media_new.title;
        //media.year = metadata.year;
        //media.genres = metadata.genres;
        media.refreshed_at = Some(Utc::now().naive_utc());
        Ok(media)
        // Ok(media_new<db::Media>[0])
    }

    async fn get_seasons(
        &self,
        mut media: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        //  dbg!(&media);
        let meta = ctx
            .aio
            .get_meta(media.kind.clone().into(), media.imdb_id.clone().unwrap())
            .await?;
        // .context("Failed to fetch metadata")?;
        let medias: Vec<db::Media> = meta.try_into()?;
        let seasons = medias
            .into_iter()
            .filter(|x| x.kind == db::MediaKind::Season)
            .collect();
        Ok(Some(seasons))
    }

    async fn get_episodes(
        &self,
        mut media: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        Ok(None)
    }
}
