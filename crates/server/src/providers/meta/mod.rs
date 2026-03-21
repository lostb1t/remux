use crate::{AppContext, db, sdks, utils};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{debug, error, info, warn};

mod tmdb;
pub use tmdb::TmdbMetaProvider;

/// Flat relation entry returned by a provider.
pub struct MetaRelation {
    pub media: db::Media,
    pub relation: db::MediaRelation,
}

/// What a MetaProvider returns: enriched media + discovered relations.
pub struct MetaResult {
    pub media: db::Media,
    pub relations: Vec<MetaRelation>,
}

/// Enriches metadata fields on a single Media item.
/// Providers are chained in order — primary fills first, subsequent providers fill None fields.
#[async_trait]
pub trait MetaProvider: Send + Sync {
    /// Fetch metadata for the given media item.
    /// Returns `Some(MetaResult)` if found, `None` if not applicable/not found.
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>>;
}

/// Discovers the tree structure (seasons/episodes) for a series.
#[async_trait]
pub trait TreeSyncProvider: Send + Sync {
    async fn get_seasons(
        &self,
        series: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;
    async fn get_episodes(
        &self,
        season: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;
}

/// Orchestrates metadata enrichment and tree syncing across multiple providers.
pub struct MetaProviderService {
    meta_providers: Vec<Box<dyn MetaProvider>>,
    tree_providers: Vec<Box<dyn TreeSyncProvider>>,
}

impl Default for MetaProviderService {
    fn default() -> Self {
        Self {
            meta_providers: vec![
                Box::new(AioMetaProvider),
                Box::new(TmdbMetaProvider),
            ],
            tree_providers: vec![Box::new(AioTreeSyncProvider)],
        }
    }
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
    /// Primary provider (index 0) replaces when force_refresh=true; all others only fill None fields.
    pub async fn apply_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Result<()> {
        // Only run providers for kinds that have metadata sources.
        if !matches!(
            media.kind,
            db::MediaKind::Movie
                | db::MediaKind::Series
                | db::MediaKind::Season
                | db::MediaKind::Episode
        ) {
            return Ok(());
        }

        for (i, provider) in self.meta_providers.iter().enumerate() {
            // Primary provider respects force_refresh; subsequent providers are gap-fillers.
            let replace = i == 0 && force_refresh;

            match provider.fetch(media, ctx).await {
                Ok(Some(result)) => {
                    merge_media(media, &result.media, replace);
                    apply_title_format(media);

                    if matches!(
                        media.kind,
                        db::MediaKind::Movie | db::MediaKind::Series
                    ) && !result.relations.is_empty()
                    {
                        let (rel_media, rels): (Vec<_>, Vec<_>) = result
                            .relations
                            .into_iter()
                            .map(|r| (r.media, r.relation))
                            .unzip();
                        if let Err(e) = db::Media::insert(&ctx.db, &rel_media)
                            .await
                            .and(db::MediaRelation::upsert(&ctx.db, &rels).await)
                        {
                            warn!(id = %media.id, error = %e, "failed to persist relations");
                        }
                    }
                }
                Ok(None) => continue,
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
    pub async fn sync_tree(
        &self,
        series: &mut db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        if series.kind != db::MediaKind::Series {
            return Ok(vec![]);
        }

        let mut children = vec![];
        let existing_seasons = series.seasons(&ctx.db).await.unwrap_or_default();
        let existing_season_idxs: Vec<i64> =
            existing_seasons.iter().filter_map(|s| s.idx).collect();

        for tree_provider in &self.tree_providers {
            let new_seasons = match tree_provider.get_seasons(series, ctx).await {
                Ok(s) => s,
                Err(e) => {
                    warn!(id = %series.id, error = %e, "failed to get seasons");
                    continue;
                }
            };
            let filtered_seasons: Vec<db::Media> = new_seasons
                .into_iter()
                .filter(|s| {
                    s.idx
                        .map(|idx| !existing_season_idxs.contains(&idx))
                        .unwrap_or(false)
                })
                .collect();

            let all_seasons: Vec<db::Media> = existing_seasons
                .iter()
                .cloned()
                .chain(filtered_seasons.into_iter())
                .collect();

            for season in all_seasons {
                children.push(season.clone());

                match tree_provider.get_episodes(&season, ctx).await {
                    Ok(episodes) => {
                        children.extend(episodes);
                    }
                    Err(e) => {
                        warn!(id = %season.id, error = %e, "failed to get episodes");
                    }
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
    /// Runs up to 10 items concurrently. Errors on individual items are logged and skipped.
    /// If `save` is true, the resulting batch is upserted to the DB before returning.
    pub async fn process(
        &self,
        media: Vec<db::Media>,
        ctx: &AppContext,
        force_refresh: bool,
        save: bool,
    ) -> Result<Vec<db::Media>> {
        let total = media.len();
        let counter = AtomicUsize::new(0);

        let results: Vec<Vec<db::Media>> = stream::iter(media)
            .map(|mut m| {
                let counter = &counter;
                async move {
                    let mut batch = vec![];
                        debug!(id = %m.id, title = %m.title, "processing");
                    if let Err(e) = self.apply_meta(&mut m, ctx, force_refresh).await {
                        warn!(id = %m.id, title = %m.title, error = %e, "failed to apply metadata, skipping");
                        batch.push(m);
                        return batch;
                    }

                    if m.kind == db::MediaKind::Series {
                        batch.push(m.clone());
                        match self.sync_tree(&mut m, ctx).await {
                            Ok(children) => {
                                let i = counter.fetch_add(1, Ordering::Relaxed) + 1;
                                //info!(title = %m.title, seasons_episodes = children.len(), "[{}/{}] synced series tree", i, total);
                                batch.extend(children);
                            }
                            Err(e) => {
                                warn!(id = %m.id, title = %m.title, error = %e, "failed to sync tree, skipping");
                            }
                        }
                    } else {
                        batch.push(m);
                    }

                    batch
                }
            })
            .buffer_unordered(10)
            .collect()
            .await;

        let batch: Vec<db::Media> = results.into_iter().flatten().collect();

        if save && !batch.is_empty() {
            db::Media::upsert(&ctx.db, &batch).await?;
        }

        Ok(batch)
    }
}

/// Merge fields from `source` into `target`.
/// If `replace` is true, overwrites existing values; otherwise only fills `None`/empty fields.
fn merge_media(target: &mut db::Media, source: &db::Media, replace: bool) {
    macro_rules! fill {
        ($field:ident) => {
            if replace || target.$field.is_none() {
                if source.$field.is_some() {
                    target.$field = source.$field.clone();
                }
            }
        };
    }
    if replace || target.title.is_empty() {
        if !source.title.is_empty() {
            target.title = source.title.clone();
        }
    }
    fill!(poster);
    fill!(description);
    fill!(released_at);
    fill!(runtime);
    fill!(rating_audience);
    fill!(certification);
    fill!(logo);
    fill!(backdrop);
    fill!(trailers);
    fill!(digital_released_at);
    fill!(status);
}

/// Reformat title for Season/Episode items based on index metadata.
fn apply_title_format(media: &mut db::Media) {
    if media.kind == db::MediaKind::Season {
        media.title = format!("Season {}", media.idx.unwrap_or(1));
    }
    if media.kind == db::MediaKind::Episode {
        if let Some(ep) = media.idx {
            media.title = match media.parent_idx {
                Some(s) => format!("S{}E{} - {}", s, ep, media.title),
                None => format!("E{} - {}", ep, media.title),
            };
        }
    }
}

pub struct AioMetaProvider;

#[async_trait]
impl MetaProvider for AioMetaProvider {
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        let imdb_id = media.series_imdb_id.clone().or(media.imdb_id.clone());

        let imdb_id = match imdb_id {
            Some(id) => id,
            None => return Ok(None),
        };

        let meta = crate::aio::AioService::from_settings(&ctx.db)
            .await?
            .get_meta(db::media_kind_to_aio(&media.kind), imdb_id)
            .await?;

        let meta_raw = meta.clone();
        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
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
            // Build relations for movies/series
            let relations =
                if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
                    build_relations(media, &meta_raw)
                } else {
                    vec![]
                };

            Ok(Some(MetaResult {
                media: found_media,
                relations,
            }))
        } else {
            Ok(None)
        }
    }
}

pub struct AioTreeSyncProvider;

#[async_trait]
impl TreeSyncProvider for AioTreeSyncProvider {
    async fn get_seasons(
        &self,
        series: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let imdb_id = match series.imdb_id.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let meta = crate::aio::AioService::from_settings(&ctx.db)
            .await?
            .get_meta(db::media_kind_to_aio(&series.kind), imdb_id)
            .await?;

        let meta_clone = meta.clone();
        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
        let seasons = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Season {
                    x.parent_id = Some(series.id);
                    x.poster = x.idx.and_then(|idx| meta_clone.get_season_poster(idx));
                    x.title = format!("Season {}", x.idx.unwrap_or(1));
                    x.refreshed_at = Some(Utc::now().naive_utc());
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(seasons)
    }

    async fn get_episodes(
        &self,
        season: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let imdb_id = match season.series_imdb_id.clone() {
            Some(id) => id,
            None => return Ok(vec![]),
        };

        let meta = crate::aio::AioService::from_settings(&ctx.db)
            .await?
            .get_meta(db::media_kind_to_aio(&season.kind), imdb_id)
            .await?;

        let medias: Vec<db::Media> = db::aio_meta_to_medias(meta)?;
        let episodes = medias
            .into_iter()
            .filter_map(|mut x| {
                if x.kind == db::MediaKind::Episode && x.parent_idx == season.idx {
                    x.parent_id = Some(season.id);
                    if let Some(episode_num) = x.idx {
                        if let Some(season_num) = x.parent_idx {
                            x.title = format!(
                                "S{}E{} - {}",
                                season_num, episode_num, x.title
                            );
                        } else {
                            x.title = format!("E{} - {}", episode_num, x.title);
                        }
                    }
                    x.refreshed_at = Some(Utc::now().naive_utc());
                    Some(x)
                } else {
                    None
                }
            })
            .collect();
        Ok(episodes)
    }
}

/// Build Person/Genre MetaRelation entries from AIO metadata.
fn build_relations(media: &db::Media, meta: &sdks::aio::Meta) -> Vec<MetaRelation> {
    let mut relations: Vec<MetaRelation> = Vec::new();

    // Genres
    if let Some(genres) = meta.genre.as_ref().or(meta.genres.as_ref()) {
        for genre_name in genres {
            let genre_id =
                utils::get_stable_uuid(format!("genre:{}", genre_name.to_lowercase()));
            relations.push(MetaRelation {
                media: db::Media {
                    id: genre_id,
                    title: genre_name.clone(),
                    kind: db::MediaKind::Genre,
                    aio_id: Some(format!("genre:{}", genre_name.to_lowercase())),
                    ..Default::default()
                },
                relation: db::MediaRelation {
                    left_media_id: media.id,
                    right_media_id: genre_id,
                    role: None,
                    ..Default::default()
                },
            });
        }
    }

    // Cast (actors)
    if let Some(extras) = &meta.app_extras {
        if let Some(cast) = &extras.cast {
            for (i, member) in cast.iter().enumerate() {
                if let Some(name) = &member.name {
                    let person_id = utils::get_stable_uuid(format!(
                        "person:{}",
                        name.to_lowercase()
                    ));
                    relations.push(MetaRelation {
                        media: db::Media {
                            id: person_id,
                            title: name.clone(),
                            kind: db::MediaKind::Person,
                            poster: member.photo.clone(),
                            aio_id: Some(format!("person:{}", name.to_lowercase())),
                            ..Default::default()
                        },
                        relation: db::MediaRelation {
                            left_media_id: media.id,
                            right_media_id: person_id,
                            weight: Some(i as i64),
                            role: Some(db::RelationRole::Actor),
                            ..Default::default()
                        },
                    });
                }
            }
        }

        // Directors
        if let Some(directors) = &extras.directors {
            for (i, name) in directors.iter().enumerate() {
                let person_id =
                    utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
                relations.push(MetaRelation {
                    media: db::Media {
                        id: person_id,
                        title: name.clone(),
                        kind: db::MediaKind::Person,
                        aio_id: Some(format!("person:{}", name.to_lowercase())),
                        ..Default::default()
                    },
                    relation: db::MediaRelation {
                        left_media_id: media.id,
                        right_media_id: person_id,
                        weight: Some(i as i64),
                        role: Some(db::RelationRole::Director),
                        ..Default::default()
                    },
                });
            }
        }

        // Writers
        if let Some(writers) = &extras.writers {
            for (i, name) in writers.iter().enumerate() {
                let person_id =
                    utils::get_stable_uuid(format!("person:{}", name.to_lowercase()));
                relations.push(MetaRelation {
                    media: db::Media {
                        id: person_id,
                        title: name.clone(),
                        kind: db::MediaKind::Person,
                        aio_id: Some(format!("person:{}", name.to_lowercase())),
                        ..Default::default()
                    },
                    relation: db::MediaRelation {
                        left_media_id: media.id,
                        right_media_id: person_id,
                        weight: Some(i as i64),
                        role: Some(db::RelationRole::Writer),
                        ..Default::default()
                    },
                });
            }
        }
    }

    relations
}
