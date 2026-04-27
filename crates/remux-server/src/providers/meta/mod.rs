use crate::{AppContext, db, sdks};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

pub(crate) mod aio;
mod tmdb;
pub use aio::{AioMetaProvider, AioTreeSyncProvider};
pub use tmdb::{TmdbMetaProvider, tmdb_remote_images};

/// Flat relation entry returned by a provider.
pub struct MetaRelation {
    pub media: db::Media,
    pub relation: db::MediaRelation,
}

/// What a MetaProvider returns: enriched media + discovered relations.
pub struct MetaResult {
    pub media: db::Media,
    pub relations: Vec<MetaRelation>,
    /// season_number → poster URL, populated by providers that have this data (e.g. TMDB).
    pub season_posters: HashMap<i64, String>,
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
            meta_providers: vec![Box::new(AioMetaProvider), Box::new(TmdbMetaProvider)],
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
    ) -> Result<HashMap<i64, String>> {
        let mut season_posters: HashMap<i64, String> = HashMap::new();

        // Only run providers for kinds that have metadata sources.
        if !matches!(
            media.kind,
            db::MediaKind::Movie
                | db::MediaKind::Series
                | db::MediaKind::Season
                | db::MediaKind::Episode
        ) {
            return Ok(season_posters);
        }

        for (i, provider) in self.meta_providers.iter().enumerate() {
            // Primary provider respects force_refresh; subsequent providers are gap-fillers.
            let replace = i == 0 && force_refresh;

            match provider.fetch(media, ctx).await {
                Ok(Some(result)) => {
                    merge_media(media, &result.media, replace);
                    apply_title_format(media);

                    // Collect season posters — later providers fill gaps left by earlier ones.
                    for (idx, url) in result.season_posters {
                        season_posters.entry(idx).or_insert(url);
                    }

                    if matches!(
                        media.kind,
                        db::MediaKind::Movie
                            | db::MediaKind::Series
                            | db::MediaKind::Episode
                    ) && !result.relations.is_empty()
                    {
                        let (rel_media, rels): (Vec<_>, Vec<_>) = result
                            .relations
                            .into_iter()
                            .map(|r| (r.media, r.relation))
                            .unzip();
                        if replace {
                            db::MediaRelation::delete_by_left_id(&ctx.db, &media.id)
                                .await
                                .ok();
                        }

                        if let Err(e) = db::Media::upsert(&ctx.db, &rel_media)
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
        Ok(season_posters)
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
                    Ok(eps) => {
                        children.extend(eps);
                    }
                    Err(e) => {
                        warn!(id = %season.id, error = %e, "failed to get episodes");
                    }
                }
            }

            // Use first tree provider that returns data
            if !children.is_empty() {
                // If we have children (episodes), also build and save their relations (cast/crew)
                if let Ok(aio) = crate::aio::AioService::from_settings(&ctx.db).await {
                    let media_type = sdks::aio::MediaType::Series; // series.kind to aio
                    if let Some(media_id) = &series.media_id {
                        if let Ok(series_meta) =
                            aio.get_meta(media_type, media_id.clone()).await
                        {
                            if let Some(ref episodes) = series_meta.videos {
                                let mut all_relations = Vec::new();
                                for media in &children {
                                    if media.kind == db::MediaKind::Episode {
                                        if let Some(ep_id) = &media.media_id {
                                            if let Some(meta_ep) = episodes.iter().find(
                                                |e: &&sdks::aio::Episode| {
                                                    &e.id == ep_id
                                                },
                                            ) {
                                                let rels = aio::build_episode_relations(
                                                    media, meta_ep,
                                                );
                                                all_relations.extend(rels);
                                            }
                                        }
                                    }
                                }
                                if !all_relations.is_empty() {
                                    let persons: Vec<db::Media> = all_relations
                                        .iter()
                                        .map(|r| r.media.clone())
                                        .collect();
                                    db::Media::upsert(&ctx.db, &persons).await?;
                                    let relations: Vec<db::MediaRelation> =
                                        all_relations
                                            .iter()
                                            .map(|r| r.relation.clone())
                                            .collect();
                                    db::MediaRelation::upsert(&ctx.db, &relations)
                                        .await?;
                                }
                            }
                        }
                    }
                }
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
        let counter = AtomicUsize::new(0);

        let results: Vec<Vec<db::Media>> = stream::iter(media)
            .map(|mut m| {
                let counter = &counter;
                async move {
                    let mut batch = vec![];
                        debug!(id = %m.id, title = %m.title, "processing");
                    let season_posters = match self.apply_meta(&mut m, ctx, force_refresh).await {
                        Ok(sp) => sp,
                        Err(e) => {
                            warn!(id = %m.id, title = %m.title, error = %e, "failed to apply metadata, skipping");
                            batch.push(m);
                            return batch;
                        }
                    };

                    if m.kind == db::MediaKind::Series {
                        batch.push(m.clone());
                        match self.sync_tree(&mut m, ctx).await {
                            Ok(mut children) => {
                                let _ = counter.fetch_add(1, Ordering::Relaxed) + 1;
                                for child in children.iter_mut() {
                                    if child.kind == db::MediaKind::Season && child.poster.is_none() {
                                        if let Some(idx) = child.idx {
                                            if let Some(url) = season_posters.get(&idx) {
                                                child.poster = Some(url.clone());
                                            }
                                        }
                                    }
                                }
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
            //info!("Seasons length: {:?}", batch.len());
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
    macro_rules! fill_image {
        ($field:ident) => {
            // Images are always overwritten if the source has a value,
            // allowing TMDB to provide hi-res versions over AIO's low-res ones.
            if source.$field.is_some() {
                target.$field = source.$field.clone();
            }
        };
    }
    if replace || target.title.is_empty() {
        if !source.title.is_empty() {
            target.title = source.title.clone();
        }
    }
    fill_image!(poster);
    fill!(description);
    fill!(released_at);
    fill!(runtime);
    fill!(rating_audience);
    fill!(certification);
    fill!(certification_age);
    fill!(country);
    fill_image!(logo);
    fill_image!(backdrop);
    fill!(trailers);
    fill!(digital_released_at);
    fill!(status);

    // External IDs are merged additively — every provider that resolves
    // a fresh TMDB / IMDB / TVDB id should contribute it without clobbering
    // the others. Without this the TMDB-resolved episode ids never reach
    // the client, leaving Anfiteatro with no ProviderIds to drive reviews,
    // remote images, or version matching.
    let mut merged_ids = target.external_ids.0.clone();
    if source.external_ids.imdb.is_some()
        && (replace || merged_ids.imdb.is_none())
    {
        merged_ids.imdb = source.external_ids.imdb.clone();
    }
    if source.external_ids.tmdb.is_some()
        && (replace || merged_ids.tmdb.is_none())
    {
        merged_ids.tmdb = source.external_ids.tmdb;
    }
    if source.external_ids.tvdb.is_some()
        && (replace || merged_ids.tvdb.is_none())
    {
        merged_ids.tvdb = source.external_ids.tvdb;
    }
    target.external_ids = sqlx::types::Json(merged_ids);
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
