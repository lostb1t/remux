use crate::{AppContext, aio, db, sdks};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use tracing::error;

pub struct MetaProviderService;

#[async_trait]
pub trait MetaProvider: Send + Sync {
    async fn apply(&self, mut media: db::Media, ctx: &AppContext) -> Result<db::Media>;

    async fn apply_many(
        &self,
        media: Vec<db::Media>,
        ctx: AppContext,
    ) -> Result<Vec<db::Media>> {
        // todo: optimize later
        let chunk_size = 10;
        
                                  
                                  let results = stream::iter(media)
            .map(|mut m| {
                let ctx = ctx.clone();
                let media_title = m.title.clone();
                            //    dbg!(&media_title);
                async move {
                    let mut media_list = match self.apply(m.clone(), &ctx).await {
                        Ok(media_vec) => vec![media_vec],
                        Err(e) => {
                            error!("Failed to process media '{}': {}", media_title, e);
                            Vec::new()
                        }
                    };
                    //return Ok::<Vec<db::Media>, anyhow::Error>(media_list);
                    if m.kind == db::MediaKind::Series {
                        let mut seasons = m.seasons(&ctx.db).await.unwrap_or_default();

                        if let Some(mut seasons_new) =
                            self.get_seasons(m.clone(), ctx.clone()).await.ok().flatten()
                        {
                            let existing_idxs: Vec<i64> =
                                seasons.iter().filter_map(|s| s.idx).collect();
                            
                            seasons_new.retain(|x| {
                                x.idx
                                    .map(|idx| !existing_idxs.contains(&idx))
                                    .unwrap_or(false)
                            });

                            seasons.extend(seasons_new.clone());
                                                              for s in &seasons {
                                    media_list.push(self.apply(s.clone(), &ctx).await.unwrap_or_default());
                                  }
                            
                            //seasons = this.apply_many(seasons, ctx.clone()).await.unwrap_or_default();

                            //media_list.extend(seasons.clone());

                            for season in seasons {
                                if let Some(episodes) =
                                    self.get_episodes(season.clone(), ctx.clone()).await.ok().flatten()
                                {
                                  //dbg!(&episodes.len());
                                  for ep in episodes {
                                    media_list.push(self.apply(ep, &ctx).await.unwrap_or_default());
                                  }
                                  //  let episodes = this.apply_many(episodes, ctx.clone()).await.unwrap_or_default();
                                  //  media_list.extend(episodes);
                                }
                            }
                        }
                    }
                    
                    Ok::<Vec<db::Media>, anyhow::Error>(media_list)
                }
            })
            .buffer_unordered(chunk_size)
            .collect::<Vec<_>>()
            .await
            .into_iter()
            .collect::<Result<Vec<Vec<db::Media>>, _>>()?
            .into_iter()
            .flatten()
            .collect();

        Ok(results)
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
    async fn apply(&self, mut media: db::Media, ctx: &AppContext) -> Result<db::Media> {
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
            db::MediaKind::Movie => {
                medias.into_iter().find(|x| x.kind == db::MediaKind::Movie)
            }
            db::MediaKind::Series => {
                medias.into_iter().find(|x| x.kind == db::MediaKind::Series)
            }
            db::MediaKind::Season => {
                let idx = media.idx;
                medias
                    .into_iter()
                    .find(|x| x.kind == db::MediaKind::Season && x.idx == idx)
            }
            db::MediaKind::Episode => {
                let idx = media.idx;
                medias
                    .into_iter()
                    .find(|x| x.kind == db::MediaKind::Episode && x.idx == idx)
            }
            _ => None,
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
                        media.title = format!(
                            "S{}E{} - {}",
                            season_num, episode_num, media.title
                        );
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
                    x.poster =
                        media.idx.and_then(|idx| meta_clone.get_season_poster(idx));
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
        mut season: db::Media,
        ctx: AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        let meta = ctx
            .aio
            .get_meta(
                season.kind.clone().into(),
                season.series_imdb_id.clone().unwrap(),
            )
            .await?;

        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = meta.try_into()?;
        let episodes = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Episode && x.parent_idx == season.idx {
                    x.parent_id = Some(season.id);
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(Some(episodes))
    }
}
