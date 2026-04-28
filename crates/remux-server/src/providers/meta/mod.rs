use crate::{AppContext, db, sdks};
use anyhow::Result;
use async_trait::async_trait;
use chrono::Utc;
use futures::stream::{self, StreamExt};
use tracing::{debug, error, warn};
use uuid::Uuid;

pub(crate) mod aio;
mod deezer;
mod tmdb;
mod ytdlp;
pub use aio::{AioHierarchySyncProvider, AioMetaProvider};
pub use deezer::{DeezerHierarchySyncProvider, DeezerMusicMetaProvider};
pub use tmdb::{TmdbMetaProvider, tmdb_remote_images};
pub use ytdlp::YtDlpMusicMetaProvider;

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

    fn supported_kinds(&self) -> &'static [db::MediaKind];

    fn supports(&self, kind: &db::MediaKind) -> bool {
        self.supported_kinds().contains(kind)
    }

    fn can_refresh(&self, media: &db::Media) -> bool {
        self.supports(&media.kind)
    }

    async fn refresh_tree_meta(
        &self,
        _series: &db::Media,
        _children: &mut [db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }
}

/// Discovers hierarchy children under a root media item.
#[async_trait]
pub trait HierarchySyncProvider: Send + Sync {
    fn supported_root_kinds(&self) -> &'static [db::MediaKind];

    fn supports_root(&self, kind: &db::MediaKind) -> bool {
        self.supported_root_kinds().contains(kind)
    }

    async fn sync_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;

    async fn persist_children_metadata(
        &self,
        _root: &db::Media,
        _children: &[db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }
}

/// Orchestrates metadata enrichment and tree syncing across multiple providers.
pub struct MetaProviderService {
    meta_providers: Vec<Box<dyn MetaProvider>>,
    hierarchy_providers: Vec<Box<dyn HierarchySyncProvider>>,
}

impl Default for MetaProviderService {
    fn default() -> Self {
        Self {
            meta_providers: vec![
                Box::new(AioMetaProvider),
                Box::new(TmdbMetaProvider),
                Box::new(DeezerMusicMetaProvider::default()),
                Box::new(YtDlpMusicMetaProvider::default()),
            ],
            hierarchy_providers: vec![
                Box::new(AioHierarchySyncProvider),
                Box::new(DeezerHierarchySyncProvider::default()),
            ],
        }
    }
}

impl MetaProviderService {
    pub fn new(
        meta_providers: Vec<Box<dyn MetaProvider>>,
        hierarchy_providers: Vec<Box<dyn HierarchySyncProvider>>,
    ) -> Self {
        Self {
            meta_providers,
            hierarchy_providers,
        }
    }

    /// Refresh metadata on a single item using providers in order.
    /// Primary provider (index 0) replaces when force_refresh=true; all others only fill None fields.
    pub async fn refresh_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Result<()> {
        if !self
            .meta_providers
            .iter()
            .any(|provider| provider.can_refresh(media))
        {
            return Ok(());
        }

        let mut first_applicable = true;
        for provider in &self.meta_providers {
            if !provider.can_refresh(media) {
                continue;
            }

            // Primary applicable provider respects force_refresh; subsequent providers are gap-fillers.
            let replace = first_applicable && force_refresh;
            first_applicable = false;

            match provider.fetch(media, ctx).await {
                Ok(Some(result)) => {
                    merge_media(media, &result.media, replace);
                    apply_title_format(media);

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
        Ok(())
    }

    async fn refresh_tree_meta(
        &self,
        series: &db::Media,
        children: &mut [db::Media],
        ctx: &AppContext,
    ) {
        for provider in &self.meta_providers {
            if !provider.supports(&series.kind) {
                continue;
            }
            if let Err(e) = provider.refresh_tree_meta(series, children, ctx).await {
                warn!(id = %series.id, error = %e, "failed to refresh tree metadata");
            }
        }
    }

    /// Sync the hierarchy under a root media item.
    /// Returns all discovered child media that should be upserted.
    pub async fn sync_hierarchy(
        &self,
        root: &mut db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        if !self
            .hierarchy_providers
            .iter()
            .any(|provider| provider.supports_root(&root.kind))
        {
            return Ok(vec![]);
        }

        for provider in &self.hierarchy_providers {
            if !provider.supports_root(&root.kind) {
                continue;
            }

            let mut children = match provider.sync_children(root, ctx).await {
                Ok(children) => children,
                Err(e) => {
                    warn!(id = %root.id, error = %e, "failed to sync hierarchy");
                    continue;
                }
            };

            // Use first tree provider that returns data
            if !children.is_empty() {
                self.refresh_tree_meta(root, &mut children, ctx).await;

                if let Err(e) = provider
                    .persist_children_metadata(root, &children, ctx)
                    .await
                {
                    warn!(id = %root.id, error = %e, "failed to persist hierarchy metadata");
                }

                return Ok(children);
            }
        }

        Ok(vec![])
    }

    async fn process_item(
        &self,
        mut media: db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Vec<db::Media> {
        let mut batch = vec![];
        debug!(id = %media.id, title = %media.title, "processing");

        match self.refresh_meta(&mut media, ctx, force_refresh).await {
            Ok(()) => {}
            Err(e) => {
                warn!(id = %media.id, title = %media.title, error = %e, "failed to refresh metadata, skipping");
                batch.push(media);
                return batch;
            }
        };

        if self
            .hierarchy_providers
            .iter()
            .any(|provider| provider.supports_root(&media.kind))
        {
            batch.push(media.clone());
            match self.sync_hierarchy(&mut media, ctx).await {
                Ok(children) => {
                    batch.extend(children);
                }
                Err(e) => {
                    warn!(id = %media.id, title = %media.title, error = %e, "failed to sync hierarchy, skipping");
                }
            }
        } else {
            batch.push(media);
        }

        batch
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
        let results: Vec<Vec<db::Media>> = stream::iter(media)
            .map(|media| self.process_item(media, ctx, force_refresh))
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
    if source.external_ids.imdb.is_some() && (replace || merged_ids.imdb.is_none()) {
        merged_ids.imdb = source.external_ids.imdb.clone();
    }
    if source.external_ids.tmdb.is_some() && (replace || merged_ids.tmdb.is_none()) {
        merged_ids.tmdb = source.external_ids.tmdb;
    }
    if source.external_ids.tvdb.is_some() && (replace || merged_ids.tvdb.is_none()) {
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
