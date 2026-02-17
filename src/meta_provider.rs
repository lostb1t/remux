use crate::{AppContext, aio, db, sdks};
use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use tracing::error;

/// Enriches metadata fields on a single Media item.
/// Providers are chained in order — first provider that finds data wins.
#[async_trait]
pub trait MetaProvider: Send + Sync {
    /// Apply metadata to a media item. Returns `true` if metadata was found.
    async fn apply(&self, media: &mut db::Media, ctx: &AppContext) -> Result<bool>;
}

/// Discovers the tree structure (seasons/episodes) for a series.
#[async_trait]
pub trait TreeSyncProvider: Send + Sync {
    async fn get_seasons(&self, series: &db::Media, ctx: &AppContext) -> Result<Vec<db::Media>>;
    async fn get_episodes(&self, season: &db::Media, ctx: &AppContext) -> Result<Vec<db::Media>>;
}

/// Orchestrates metadata enrichment and tree syncing across multiple providers.
pub struct MetaProviderService {
    meta_providers: Vec<Box<dyn MetaProvider>>,
    tree_providers: Vec<Box<dyn TreeSyncProvider>>,
}

impl MetaProviderService {
    pub fn new(
        meta_providers: Vec<Box<dyn MetaProvider>>,
        tree_providers: Vec<Box<dyn TreeSyncProvider>>,
    ) -> Self {
        Self {
            meta_providers,
            tree_providers,
        }
    }

    /// Enrich metadata on a single item using providers in order.
    pub async fn apply_meta(&self, media: &mut db::Media, ctx: &AppContext) -> Result<()> {
        for provider in &self.meta_providers {
            match provider.apply(media, ctx).await {
                Ok(true) => break,
                Ok(false) => continue,
                Err(e) => {
                    error!("meta provider error: {e}");
                    continue;
                }
            }
        }
        media.refreshed_at = Some(Utc::now().naive_utc());
        Ok(())
    }

    /// Sync the tree for a series: discover new seasons and episodes.
    /// Returns all child media (seasons + episodes) that should be upserted.
    pub async fn sync_tree(&self, series: &mut db::Media, ctx: &AppContext) -> Result<Vec<db::Media>> {
        if series.kind != db::MediaKind::Series {
            return Ok(vec![]);
        }

        let mut children = vec![];
        let existing_seasons = series.seasons(&ctx.db).await.unwrap_or_default();
        let existing_season_idxs: Vec<i64> = existing_seasons.iter().filter_map(|s| s.idx).collect();

        for tree_provider in &self.tree_providers {
            let mut new_seasons = tree_provider.get_seasons(series, ctx).await?;
            new_seasons.retain(|s| {
                s.idx.map(|idx| !existing_season_idxs.contains(&idx)).unwrap_or(false)
            });

            let all_seasons: Vec<db::Media> = existing_seasons
                .iter()
                .cloned()
                .chain(new_seasons.into_iter())
                .collect();

            for mut season in all_seasons {
                self.apply_meta(&mut season, ctx).await?;
                children.push(season.clone());

                let episodes = tree_provider.get_episodes(&season, ctx).await?;
                for mut ep in episodes {
                    self.apply_meta(&mut ep, ctx).await?;
                    children.push(ep);
                }
            }

            // Use first tree provider that returns data
            if !children.is_empty() {
                break;
            }
        }

        Ok(children)
    }

    /// Process a batch of media: enrich metadata and sync trees for series.
    pub async fn process(&self, media: Vec<db::Media>, ctx: &AppContext) -> Result<Vec<db::Media>> {
        let mut results = vec![];

        for mut m in media {
            if m.kind == db::MediaKind::Series {
                self.apply_meta(&mut m, ctx).await?;
                results.push(m.clone());
                let children = self.sync_tree(&mut m, ctx).await?;
                results.extend(children);
            } else {
                self.apply_meta(&mut m, ctx).await?;
                results.push(m);
            }
        }

        Ok(results)
    }
}

// --- Aio implementations ---

pub struct AioMetaProvider;

#[async_trait]
impl MetaProvider for AioMetaProvider {
    async fn apply(&self, media: &mut db::Media, ctx: &AppContext) -> Result<bool> {
        let imdb_id = media
            .series_imdb_id
            .clone()
            .or(media.imdb_id.clone());

        let imdb_id = match imdb_id {
            Some(id) => id,
            None => return Ok(false),
        };

        let meta = ctx
            .aio
            .get_meta(media.kind.clone().into(), imdb_id)
            .await?;

        let medias: Vec<db::Media> = meta.try_into()?;
        let found = match media.kind {
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

        if let Some(found_media) = found {
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

            if media.kind == db::MediaKind::Season {
                media.title = format!("Season {}", media.idx.unwrap_or(1));
            }

            if media.kind == db::MediaKind::Episode {
                if let Some(episode_num) = media.idx {
                    if let Some(season_num) = media.parent_idx {
                        media.title =
                            format!("S{}E{} - {}", season_num, episode_num, media.title);
                    } else {
                        media.title = format!("E{} - {}", episode_num, media.title);
                    }
                }
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }
}

pub struct AioTreeSyncProvider;

#[async_trait]
impl TreeSyncProvider for AioTreeSyncProvider {
    async fn get_seasons(&self, series: &db::Media, ctx: &AppContext) -> Result<Vec<db::Media>> {
        let imdb_id = match series.imdb_id.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let meta = ctx
            .aio
            .get_meta(series.kind.clone().into(), imdb_id)
            .await?;

        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = meta.try_into()?;
        let seasons = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Season {
                    x.parent_id = Some(series.id);
                    x.poster = x.idx.and_then(|idx| meta_clone.get_season_poster(idx));
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(seasons)
    }

    async fn get_episodes(&self, season: &db::Media, ctx: &AppContext) -> Result<Vec<db::Media>> {
        let imdb_id = match season.series_imdb_id.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let meta = ctx
            .aio
            .get_meta(season.kind.clone().into(), imdb_id)
            .await?;

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
        Ok(episodes)
    }
}
