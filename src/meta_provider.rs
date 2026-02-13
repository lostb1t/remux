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
    let chunk_size = 5;
    let this = self.clone();

    // Process media in parallel, with a concurrency limit of `chunk_size`
    let results = stream::iter(media)
        .map(|m| {
            let ctx = ctx.clone();
            let this = this.clone();
            let media_title = m.title.clone();
            async move {
                match this.refresh_tree(m, ctx).await {
                    Ok(media_vec) => Ok::<Vec<db::Media>, anyhow::Error>(media_vec),
                    Err(e) => {
                        error!("Failed to process media '{}': {}", media_title, e);
                        Ok(Vec::new()) // Return empty vec on error, or handle as needed
                    }
                }
            }
        })
        .buffer_unordered(chunk_size) // Limit concurrency
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<Vec<_>, _>>()? // Flatten results
        .into_iter()
        .flatten()
        .collect();

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

          let existing_seasons = db::Media::get_by_filter(
                &ctx.db,
                &db::MediaFilter {
                    parent_id: Some(media.id),
                    kind: Some(vec![db::MediaKind::Season]),
                    ..Default::default()
                }
            ).await?.records;
            
            if let Some(mut seasons_new) = self.get_seasons(media.clone(), ctx.clone()).await? {

               // let existing_idxs: Vec<i64> = existing_seasons.iter().filter_map(|s| s.idx).collect();
                                                                          //dbg!(&seasons_new); 
              //  seasons_new.retain(|x| x.idx.map(|idx| existing_idxs.contains(&idx)).unwrap_or(false));

                let seasons_with_meta = self.apply_many(seasons_new.clone(), ctx.clone()).await?;

                all_media.extend(seasons_with_meta);
                
                // Process episodes for each season
                for season in seasons_new {
                    if let Some(episodes) = self.get_episodes(season.clone(), ctx.clone()).await? {

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
        //return Ok(media);
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

        //let media_new: db::Media = meta.try_into()?;
        let medias: Vec<db::Media> = meta.try_into()?;
        let media_new = match media.kind {
          db::MediaKind::Movie => medias.into_iter().find(|x| x.kind == db::MediaKind::Movie),
          db::MediaKind::Series => medias.into_iter().find(|x| x.kind == db::MediaKind::Series),
          db::MediaKind::Season => {
              let idx = media.idx;
              medias.into_iter().find(|x| x.kind == db::MediaKind::Season && x.idx == idx)
          },
          db::MediaKind::Episode => {
              let idx = media.idx;
              medias.into_iter().find(|x| x.kind == db::MediaKind::Episode && x.idx == idx)
          },
          _ => None
        };
        
        if let Some(found_media) = media_new {
            media.title = found_media.title;
            media.poster = found_media.poster;
            media.description = found_media.description;
            media.released_at = found_media.released_at;
            media.runtime = found_media.runtime;
            media.rating_audience = found_media.rating_audience;
            media.certification = found_media.certification;
            media.logo = found_media.logo;
            media.backdrop = found_media.backdrop;
            media.trailers = found_media.trailers;
            
            // Special handling for seasons and episodes
            if media.kind == db::MediaKind::Season {
                media.title = format!("Season {}", media.idx.unwrap_or(1));
            }
            
            if media.kind == db::MediaKind::Episode {
                if let Some(episode_num) = media.idx {
                    if let Some(season_num) = media.parent_idx {
                        media.title = format!("S{}E{} - {}", season_num, episode_num, media.title);
                    } else {
                        media.title = format!("E{} - {}", episode_num, media.title);
                    }
                }
            }
        }
        
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
        //return Ok(None);
        let meta = ctx
            .aio
            .get_meta(media.kind.clone().into(), media.imdb_id.clone().unwrap())
            .await?;

        // .context("Failed to fetch metadata")?;
        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = meta.try_into()?;
        let seasons = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Season {
                    x.parent_id = Some(media.id);
                    x.poster = media.idx.and_then(|idx| meta_clone.get_season_poster(idx));
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(Some(seasons))
    }

    async fn get_episodes(
        &self,
        mut media: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        let meta = ctx
            .aio
            .get_meta(media.kind.clone().into(), media.series_imdb_id.clone().unwrap())
            .await?;

        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = meta.try_into()?;
        let episodes = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Episode && x.parent_idx == media.idx {
                    x.parent_id = Some(media.id);
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(Some(episodes))
    }
}