//! Unified addon abstraction. Each addon kind declares which resources ×
//! media types it serves; user-added instances are rows in the `addons` table.

pub mod addon;
pub mod deezer;
pub mod introdb;
pub mod iptv;
pub mod lrclib;
pub mod opendal;
pub mod probe;
pub mod squid;
pub mod stremio;
pub mod tmdb;
pub mod torznab;
pub mod ytdlp;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use sqlx::SqlitePool;
use std::pin::Pin;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

use crate::keyed_lock::KeyedLock;
use tracing::info;
use uuid::Uuid;

use crate::sdks;
use crate::{AppContext, api, common::ProgressReporter, db};
pub use addon::{Addon, CatalogState};
pub use remux_sdks::remux::AddonPresetRef;
use remux_sdks::remux::{LyricDto, MediaSegments, RemoteLyricInfoDto};

pub use remux_sdks::remux::{
    AddonCatalogDto, AddonDto, AddonMetadata, AddonOption, AddonOptionType,
    AddonSelectOption, CreateAddonRequest, MediaKind, UpdateAddonCatalogRequest,
    UpdateAddonRequest,
};
pub use remux_sdks::stremio::ResourceType;

#[derive(Debug, Clone)]
pub struct CatalogInfo {
    pub provider_catalog_id: String,
    pub name: String,
    /// Whether this catalog should be enabled by default (before the user changes it).
    pub default_enabled: bool,
    /// Default per-catalog item limit (before the user changes it).
    pub default_max_items: Option<i64>,
    /// Media kind for auto-created collections backed by this catalog.
    pub collection_media_kind: Option<db::CollectionMediaKind>,
}

impl CatalogInfo {
    pub fn new(
        provider_catalog_id: impl Into<String>,
        name: impl Into<String>,
    ) -> Self {
        Self {
            provider_catalog_id: provider_catalog_id.into(),
            name: name.into(),
            default_enabled: false,
            default_max_items: None,
            collection_media_kind: None,
        }
    }
}

#[async_trait]
pub trait RemoteMediaStream: Send + Sync {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>>;
}

#[derive(Debug)]
pub struct LyricSearchRequest {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Save relation links that were deferred onto `media.relations` by `apply_meta`.
/// Must be called after `db::Media::upsert` so `left_media_id` FK constraints are satisfied.
pub(crate) async fn save_pending_relations(ctx: &AppContext, items: &[db::Media]) {
    // TMDB ID is the canonical key for person rows.  Name-keyed person stubs
    // (produced by Stremio/Jellyfin addons when no TMDB ID is available) must NOT
    // be persisted — the TMDB addon will insert them with the correct TMDB-keyed UUID
    // when it enriches the parent movie/series.  Storing them now would create
    // duplicate rows alongside any existing TMDB-keyed row for the same person.
    let name_keyed_person_ids: std::collections::HashSet<Uuid> = items
        .iter()
        .filter_map(|m| m.relations.as_ref())
        .flatten()
        .filter(|(_, m)| {
            m.kind == db::MediaKind::Person && m.external_ids.tmdb.is_none()
        })
        .map(|(_, m)| m.id)
        .collect();

    // One batched upsert for all relation media (persons/genres) across the whole slice —
    // avoids opening a separate transaction per item (N items → N transactions otherwise).
    let all_rel_media: Vec<db::Media> = items
        .iter()
        .filter_map(|m| m.relations.as_ref())
        .flatten()
        .map(|(_, m)| m.clone())
        .filter(|m| !name_keyed_person_ids.contains(&m.id))
        .collect();
    if !all_rel_media.is_empty() {
        if let Err(e) = db::Media::upsert(&ctx.db, &all_rel_media).await {
            tracing::warn!(error = %e, "failed to upsert relation media batch");
        }
    }

    // Collect items that have relations, then batch delete + batch upsert
    // (replaces N×delete + N×upsert with 1 delete + 1 upsert).
    let items_with_rels: Vec<&db::Media> = items
        .iter()
        .filter(|m| m.relations.as_ref().map_or(false, |r| !r.is_empty()))
        .collect();
    if items_with_rels.is_empty() {
        return;
    }

    let all_ids: Vec<uuid::Uuid> = items_with_rels.iter().map(|m| m.id).collect();
    db::MediaRelation::delete_by_left_ids(&ctx.db, &all_ids)
        .await
        .ok();

    let all_rels: Vec<db::MediaRelation> = items_with_rels
        .iter()
        .flat_map(|m| m.relations.as_ref().unwrap().iter().map(|(r, _)| r.clone()))
        // Don't link relations that point to name-keyed person stubs.
        .filter(|r| !name_keyed_person_ids.contains(&r.right_media_id))
        .collect();
    if !all_rels.is_empty() {
        if let Err(e) = db::MediaRelation::upsert(&ctx.db, &all_rels).await {
            tracing::warn!(error = %e, "failed to upsert relations batch");
        }
    }
}

pub(crate) fn merge_media(target: &mut db::Media, source: &db::Media, replace: bool) {
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

    fill!(description);
    fill!(released_at);
    fill!(runtime);
    fill!(rating_audience);
    fill!(certification);
    fill!(certification_age);
    fill!(country);
    fill!(trailers);
    fill!(digital_released_at);
    fill!(status);

    let mut merged_ids = target.external_ids.clone();
    if source.external_ids.imdb.is_some() && (replace || merged_ids.imdb.is_none()) {
        merged_ids.imdb = source.external_ids.imdb.clone();
    }
    if source.external_ids.tmdb.is_some() && (replace || merged_ids.tmdb.is_none()) {
        merged_ids.tmdb = source.external_ids.tmdb;
    }
    if source.external_ids.tvdb.is_some() && (replace || merged_ids.tvdb.is_none()) {
        merged_ids.tvdb = source.external_ids.tvdb;
    }
    target.external_ids = merged_ids;
}

pub(crate) fn apply_title_format(media: &mut db::Media) {
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

fn apply_meta(media: &mut db::Media, mut patch: db::Media, replace: bool) {
    // Merge images onto the in-memory struct; db::Media::upsert persists them via
    // sync_from_media after the media row is committed, avoiding FK violations.
    if !patch.images.is_empty() {
        let patch_images = std::mem::take(&mut patch.images);
        let imgs = &mut media.images;
        if replace || imgs.primary.is_empty() {
            imgs.primary = patch_images.primary;
        }
        if replace || imgs.backdrop.is_empty() {
            imgs.backdrop = patch_images.backdrop;
        }
        if replace || imgs.logo.is_empty() {
            imgs.logo = patch_images.logo;
        }
        if replace || imgs.thumb.is_empty() {
            imgs.thumb = patch_images.thumb;
        }
    }

    merge_media(media, &patch, replace);
    apply_title_format(media);

    if let Some(relations) = patch.relations {
        if !relations.is_empty()
            && matches!(
                media.kind,
                db::MediaKind::Movie | db::MediaKind::Series | db::MediaKind::Episode
            )
        {
            let pending: Vec<(db::MediaRelation, db::Media)> =
                relations.into_iter().collect();
            match &mut media.relations {
                Some(existing) => existing.extend(pending),
                None => media.relations = Some(pending),
            }
        }
    }
}

// ---------------------------------------------------------------------------
impl From<crate::stream::StreamInfo> for db::Media {
    fn from(si: crate::stream::StreamInfo) -> Self {
        let title = si
            .name
            .clone()
            .or_else(|| si.description.clone())
            .unwrap_or_default();
        let probe_data = si.probe_data.clone();
        db::Media {
            kind: db::MediaKind::Stream,
            title,
            stream_info: Some(si),
            probe_data,
            ..Default::default()
        }
    }
}

// Preset registry
// ---------------------------------------------------------------------------

pub struct AddonPresetRegistration(pub fn() -> Box<dyn AddonPreset>);
inventory::collect!(AddonPresetRegistration);

pub fn registered_presets() -> Vec<Box<dyn AddonPreset>> {
    inventory::iter::<AddonPresetRegistration>
        .into_iter()
        .map(|r| (r.0)())
        .collect()
}

// ---------------------------------------------------------------------------
// AddonPreset trait — kind descriptor + sync factory
// ---------------------------------------------------------------------------

pub trait AddonPreset: Send + Sync {
    fn id(&self) -> &'static str;
    fn metadata(&self) -> AddonMetadata;
    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        config: &crate::Config,
    ) -> Result<Arc<dyn AddonKind>>;
}

// ---------------------------------------------------------------------------
// AddonKind trait — god trait with all capability methods (no-op defaults)
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AddonKind: Send + Sync {
    fn id(&self) -> &'static str;

    // index
    async fn refresh_index(
        &self,
        _ctx: &AppContext,
        _addon: &Addon,
        _progress: ProgressReporter,
    ) -> Result<()> {
        Ok(())
    }

    async fn purge_index(&self, _ctx: &AppContext, _addon: &Addon) -> Result<()> {
        Ok(())
    }

    // catalog
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![])
    }
    async fn catalog_stream(
        &self,
        _ctx: &AppContext,
        _local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        Ok(None)
    }

    // meta
    async fn meta_supports(&self, _media: &db::Media) -> bool {
        false
    }
    /// Fetch metadata for `media` and return a partial `db::Media` patch.
    /// Only the fields the addon knows about need to be populated; the caller
    /// merges the patch into the existing record via `merge_media`.
    /// Populate `patch.images` for images, `patch.relations` for people/genres.
    async fn meta_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
        _config: &api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        Ok(None)
    }

    // remote images (for manual image selection in the UI)
    async fn images_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        Ok(vec![])
    }

    // tree
    fn supports_children(&self, _root: &db::Media) -> bool {
        false
    }
    async fn get_children(
        &self,
        _root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        Ok(None)
    }

    // search
    async fn search_supports(&self, _kind: &db::MediaKind) -> bool {
        false
    }
    async fn search(
        &self,
        _kind: &db::MediaKind,
        _query: &str,
        _limit: usize,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        Ok(None)
    }
    async fn available_resources(&self) -> Vec<ResourceType> {
        vec![]
    }
    async fn available_types(&self) -> Vec<remux_sdks::stremio::MediaType> {
        vec![]
    }
    /// Returns (resources, types) in a single call. Override to avoid double fetches.
    async fn available_info(
        &self,
    ) -> (Vec<ResourceType>, Vec<remux_sdks::stremio::MediaType>) {
        (
            self.available_resources().await,
            self.available_types().await,
        )
    }

    // subtitle
    fn subtitle_supports(&self, _media: &db::Media) -> bool {
        false
    }
    async fn subtitle_fetch(
        &self,
        _media: &db::Media,
        _db: &SqlitePool,
    ) -> Result<Vec<sdks::stremio::Subtitle>> {
        Ok(vec![])
    }

    // stream
    fn stream_supports(&self, _media: &db::Media) -> bool {
        false
    }
    async fn get_streams(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        Ok(vec![])
    }

    /// Serve bytes for a stream that requires this addon's config (e.g. credentials).
    /// Only called when `StreamDescriptor::addon_id()` points to this addon.
    async fn serve_stream(
        &self,
        _descriptor: &crate::stream::StreamDescriptor,
        _headers: &axum::http::HeaderMap,
    ) -> axum_anyhow::ApiResult<axum::response::Response> {
        Err(axum_anyhow::ApiError::builder()
            .status(axum::http::StatusCode::BAD_REQUEST)
            .title("stream")
            .detail("serve_stream not implemented for this addon")
            .build())
    }

    // segment
    fn segment_supports(&self, _media: &db::Media) -> bool {
        false
    }
    async fn segment_fetch(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<MediaSegments> {
        Ok(MediaSegments::default())
    }

    // lyric
    fn lyric_provider_id(&self) -> Option<String> {
        None
    }
    async fn lyric_fetch(&self, _req: &LyricSearchRequest) -> Result<Option<LyricDto>> {
        Ok(None)
    }
    async fn lyric_search(
        &self,
        _req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        Ok(vec![])
    }
    async fn lyric_get_by_id(&self, _id: &str) -> Result<Option<LyricDto>> {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// AddonRuntime — one entry in the service Vec
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AddonRuntime {
    pub row: Addon,
    pub kind: Arc<dyn AddonKind>,
    /// Live capabilities fetched from the addon at load time (e.g. remote manifest).
    /// Acts as a hard upper bound — overrides whatever is stored in `row.types`.
    manifest_types: Option<Vec<db::MediaKind>>,
}

impl AddonRuntime {
    pub fn supports(&self, r: ResourceType) -> bool {
        self.row.resources.contains(&r)
    }

    pub fn supports_type(&self, kind: &db::MediaKind) -> bool {
        // Manifest types (if available) are the authoritative upper bound.
        // "Series" in a type list covers Episode and Season too (Stremio model).
        if let Some(ref mt) = self.manifest_types {
            if !kind_in_type_list(kind, mt) {
                return false;
            }
        }
        self.row.types.is_empty() || kind_in_type_list(kind, &self.row.types)
    }
}

fn kind_in_type_list(kind: &db::MediaKind, list: &[db::MediaKind]) -> bool {
    list.contains(kind)
        || (matches!(kind, db::MediaKind::Episode | db::MediaKind::Season)
            && list.contains(&db::MediaKind::Series))
}

// ---------------------------------------------------------------------------
// AddonService
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AddonService {
    inner: Arc<RwLock<Vec<AddonRuntime>>>,
}

impl AddonService {
    pub async fn from_db(db: &SqlitePool, config: &crate::Config) -> Result<Self> {
        let presets = registered_presets();
        let addons = Addon::list(db).await?;
        let mut runtimes = Vec::new();

        for addon in addons.into_iter().filter(|a| a.enabled) {
            let Some(preset) = presets.iter().find(|p| p.id() == addon.preset.kind)
            else {
                tracing::warn!(addon_id = %addon.id, kind = %addon.preset.kind, "skipping addon with unknown preset kind");
                continue;
            };
            match preset.from_cfg(addon.id, &addon.preset.config, config) {
                Ok(kind) => {
                    let (_, raw_types) = kind.available_info().await;
                    let manifest_types = if raw_types.is_empty() {
                        None
                    } else {
                        Some(
                            raw_types
                                .into_iter()
                                .filter_map(|t| db::MediaKind::try_from(t).ok())
                                .collect(),
                        )
                    };
                    runtimes.push(AddonRuntime {
                        row: addon,
                        kind,
                        manifest_types,
                    });
                }
                Err(e) => tracing::warn!(
                    addon_id = %addon.id,
                    kind = %addon.preset.kind,
                    error = %e,
                    "failed to instantiate addon"
                ),
            }
        }

        Ok(Self {
            inner: Arc::new(RwLock::new(runtimes)),
        })
    }

    pub async fn reload(&self, db: &SqlitePool, config: &crate::Config) -> Result<()> {
        let new = Self::from_db(db, config).await?;
        let mut guard = self.inner.write().await;
        *guard = new.inner.read().await.clone();
        Ok(())
    }

    pub async fn list(&self) -> Vec<AddonRuntime> {
        let guard = self.inner.read().await;
        guard.clone()
    }

    pub async fn get(&self, id: Uuid) -> Option<AddonRuntime> {
        let guard = self.inner.read().await;
        guard.iter().find(|r| r.row.id == id).cloned()
    }

    pub async fn catalog_addons(&self) -> Vec<AddonRuntime> {
        let guard = self.inner.read().await;
        guard
            .iter()
            .filter(|r| r.supports(ResourceType::Catalog))
            .cloned()
            .collect()
    }

    pub async fn purge_indexes(&self, ctx: &AppContext) -> Result<()> {
        let addons: Vec<AddonRuntime> = self.inner.read().await.clone();
        for runtime in &addons {
            if let Err(e) = runtime.kind.purge_index(ctx, &runtime.row).await {
                tracing::warn!(addon = %runtime.row.name, error = %e, "purge_index failed");
            }
        }
        Ok(())
    }

    pub async fn refresh_indexes(
        &self,
        ctx: &AppContext,
        progress: ProgressReporter,
    ) -> Result<()> {
        let addons: Vec<AddonRuntime> = {
            let guard = self.inner.read().await;
            guard.iter().filter(|r| r.row.enabled).cloned().collect()
        };
        let total = addons.len();
        for (idx, runtime) in addons.iter().enumerate() {
            let sub = progress.step(idx, total);
            if let Err(e) = runtime.kind.refresh_index(ctx, &runtime.row, sub).await {
                tracing::warn!(addon = %runtime.row.name, error = %e, "refresh_index failed");
            }
        }
        progress.set(100.0);
        Ok(())
    }

    pub async fn get_catalog(&self, id: Uuid) -> Option<Arc<dyn AddonKind>> {
        let guard = self.inner.read().await;
        let r = guard.iter().find(|r| r.row.id == id)?;
        if r.supports(ResourceType::Catalog) {
            Some(r.kind.clone())
        } else {
            None
        }
    }

    /// Return the tags configured for a specific catalog within an addon.
    /// `addon_uuid` is the simple (no-dashes) UUID string stored in `media_catalog_items.addon_id`.
    pub async fn catalog_tags(
        &self,
        addon_uuid: &str,
        local_cat_id: &str,
    ) -> Vec<String> {
        let Ok(id) = Uuid::parse_str(addon_uuid) else {
            return vec![];
        };
        let guard = self.inner.read().await;
        let Some(r) = guard.iter().find(|r| r.row.id == id) else {
            return vec![];
        };
        r.row
            .catalog_states()
            .get(local_cat_id)
            .map(|s| s.tags.clone())
            .unwrap_or_default()
    }

    pub async fn make_catalog_stream(
        &self,
        media_id: &str,
    ) -> Option<Box<dyn RemoteMediaStream>> {
        let rest = media_id.strip_prefix("addon:")?;
        let (uuid_str, local_id) = rest.split_once(':')?;
        let id = Uuid::parse_str(uuid_str).ok()?;
        let guard = self.inner.read().await;
        let r = guard.iter().find(|r| r.row.id == id)?;
        if !r.supports(ResourceType::Catalog) {
            return None;
        }
        let addon = r.kind.clone();
        drop(guard);
        Some(Box::new(AddonCatalogStream {
            addon,
            local_id: local_id.to_string(),
        }))
    }

    pub async fn refresh_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
        config: &api::ServerConfiguration,
    ) -> Result<()> {
        let guard = self.inner.read().await;
        let applicable: Vec<(String, Arc<dyn AddonKind>)> = {
            let mut v = Vec::new();
            for r in guard.iter().filter(|r| r.supports_type(&media.kind)) {
                if r.kind.meta_supports(media).await {
                    v.push((r.row.name.clone(), r.kind.clone()));
                }
            }
            v
        };
        drop(guard);

        if applicable.is_empty() {
            return Ok(());
        }

        let results = futures::future::join_all(
            applicable
                .iter()
                .map(|(_, addon)| addon.meta_fetch(media, ctx, config)),
        )
        .await;

        for ((name, _), result) in applicable.iter().zip(results) {
            match result {
                Ok(Some(patch)) => apply_meta(media, patch, force_refresh),
                Ok(None) => {}
                Err(e) => {
                    tracing::error!(addon = %name, error = %e, "meta addon error")
                }
            }
        }

        // Recompute stable UUID for Person once TMDB ID is resolved.
        if media.kind == db::MediaKind::Person {
            if let Some(tmdb_id) = media.external_ids.tmdb {
                media.id = crate::common::stable_media_uuid(
                    &db::MediaKind::Person,
                    &tmdb_id.to_string(),
                );
            }
        }

        media.refreshed_at = Some(chrono::Utc::now().naive_utc());
        Ok(())
    }

    /// Returns all descendants depth-first as minimal stubs — no DB writes, no enrichment.
    pub async fn get_tree(&self, root: &db::Media, ctx: &AppContext) -> Vec<db::Media> {
        let mut all = Vec::new();
        let mut queue = vec![root.clone()];
        while let Some(node) = queue.pop() {
            let guard = self.inner.read().await;
            let applicable: Vec<Arc<dyn AddonKind>> = guard
                .iter()
                .filter(|r| r.kind.supports_children(&node))
                .map(|r| r.kind.clone())
                .collect();
            drop(guard);

            for addon in &applicable {
                match addon.get_children(&node, ctx).await {
                    Ok(Some(children)) if !children.is_empty() => {
                        queue.extend(children.iter().cloned());
                        all.extend(children);
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        tracing::warn!(id = %node.id, error = %e, "get_children failed");
                        continue;
                    }
                }
            }
        }
        all
    }

    pub async fn process_meta_batch(
        &self,
        media: Vec<db::Media>,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Result<()> {
        use futures::stream::{self, StreamExt};
        let config = db::Settings::get_config(&ctx.db).await.unwrap_or_default();
        let concurrency = config.meta_concurrency.unwrap_or(5) as usize;
        let config = Arc::new(config);
        let mut stream = stream::iter(media)
            .map(|m| {
                let cfg = Arc::clone(&config);
                self.process_meta_item(m, ctx, force_refresh, cfg)
            })
            .buffer_unordered(concurrency);
        while let Some(items) = stream.next().await {
            if !items.is_empty() {
                db::Media::upsert(&ctx.db, &items).await?;
                save_pending_relations(ctx, &items).await;
            }
        }
        Ok(())
    }

    pub(crate) async fn process_meta_item(
        &self,
        mut media: db::Media,
        ctx: &AppContext,
        force_refresh: bool,
        config: Arc<api::ServerConfiguration>,
    ) -> Vec<db::Media> {
        let original_id = media.id;

        if let Err(e) = self
            .refresh_meta(&mut media, ctx, force_refresh, &config)
            .await
        {
            tracing::warn!(id = %media.id, error = %e, "failed to refresh metadata, keeping as-is");
            return vec![media];
        }

        // If this Person's ID was rewritten (name-keyed → tmdb-keyed) by refresh_meta,
        // delete the stale name-keyed row so it doesn't linger as a duplicate.
        if media.kind == db::MediaKind::Person && media.id != original_id {
            if let Err(e) = db::Media::delete(&ctx.db, &original_id).await {
                tracing::warn!(
                    old_id = %original_id,
                    new_id = %media.id,
                    error = %e,
                    "failed to delete stale name-keyed person row"
                );
            }
        }

        let tree = self.get_tree(&media, ctx).await;
        if tree.is_empty() {
            return vec![media];
        }

        // Keep a reference to the series so child items (episodes/seasons) can
        // resolve the series TMDB ID in-memory without hitting the DB.
        let grandparent = Some(Box::new(media.clone()));

        let mut items = vec![media.clone()];
        for mut item in tree {
            item.grandparent = grandparent.clone();
            if force_refresh || item.refreshed_at.is_none() {
                if let Err(e) = self
                    .refresh_meta(&mut item, ctx, force_refresh, &config)
                    .await
                {
                    tracing::warn!(id = %item.id, error = %e, "failed to refresh child meta");
                }
            }
            item.grandparent = None; // drop before pushing — not persisted, no reason to hold
            items.push(item);
        }
        items
    }

    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| r.supports(ResourceType::Search) && r.supports_type(kind))
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        for (name, addon) in addons {
            if !addon.search_supports(kind).await {
                continue;
            }
            match addon.search(kind, query, limit, ctx).await {
                Ok(Some(results)) => {
                    for m in &results {
                        ctx.store.save(
                            m.id.to_string(),
                            m.clone(),
                            Duration::from_secs(3600),
                        );
                    }
                    return Ok(results);
                }
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(addon = %name, error = %e, "search addon error")
                }
            }
        }
        Ok(vec![])
    }

    pub async fn fetch_images(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| r.supports_type(&media.kind))
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        let mut out = Vec::new();
        for (name, addon) in addons {
            match addon.images_fetch(media, ctx).await {
                Ok(images) => out.extend(images),
                Err(e) => {
                    tracing::warn!(addon = %name, error = %e, "images_fetch failed")
                }
            }
        }
        Ok(out)
    }

    pub async fn fetch_subtitles(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Vec<sdks::stremio::Subtitle> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| {
                r.supports(ResourceType::Subtitles)
                    && r.supports_type(&media.kind)
                    && r.kind.subtitle_supports(media)
            })
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        let mut subs = vec![];
        for (name, addon) in addons {
            match addon.subtitle_fetch(media, db).await {
                Ok(s) => subs.extend(s),
                Err(e) => {
                    tracing::warn!(addon = %name, error = %e, "subtitle addon failed")
                }
            }
        }
        subs
    }

    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| {
                let has_resource = r.supports(ResourceType::Stream);
                let has_type = r.supports_type(&media.kind);
                let kind_supports = r.kind.stream_supports(media);
                tracing::debug!(
                    addon = %r.row.name,
                    media_kind = ?media.kind,
                    has_resource,
                    has_type,
                    kind_supports,
                    "stream addon filter"
                );
                has_resource && has_type && kind_supports
            })
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        tracing::debug!(
            media_id = %media.id,
            media_kind = ?media.kind,
            addon_count = addons.len(),
            "resolving streams"
        );

        let tasks: Vec<_> = addons
            .into_iter()
            .map(|(name, addon)| async move {
                match addon.get_streams(media, ctx).await {
                    Ok(streams) => {
                        tracing::debug!(addon = %name, count = streams.len(), "addon returned streams");
                        streams
                    }
                    Err(e) => {
                        tracing::warn!(addon = %name, error = %e, "stream addon failed");
                        vec![]
                    }
                }
            })
            .collect();
        let all: Vec<db::Media> = futures::future::join_all(tasks)
            .await
            .into_iter()
            .flatten()
            .map(db::Media::from)
            .collect();
        Ok(all)
    }

    fn stream_dedup_key(s: &db::Media) -> Option<String> {
        match &s.stream_info.as_ref()?.descriptor {
            crate::stream::StreamDescriptor::Torrent { info_hash, .. } => {
                Some(format!("torrent:{}", info_hash.to_lowercase()))
            }
            crate::stream::StreamDescriptor::Http { url, .. } => {
                Some(format!("http:{url}"))
            }
            crate::stream::StreamDescriptor::Local(path) => {
                Some(format!("local:{}", path.display()))
            }
            crate::stream::StreamDescriptor::Opendal { addon_id, path } => {
                Some(format!("opendal:{addon_id}:{path}"))
            }
        }
    }

    pub async fn refresh_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<()> {
        const STREAMS_TTL_SECS: i64 = 60;
        static STREAM_LOCKS: KeyedLock<Uuid> = KeyedLock::new();

        // Fast path: TTL not expired — skip the lock entirely.
        let is_fresh = |refreshed: Option<chrono::NaiveDateTime>| {
            refreshed.is_some_and(|r| {
                (chrono::Utc::now().naive_utc() - r).num_seconds() < STREAMS_TTL_SECS
            })
        };
        if is_fresh(media.streams_refreshed_at) {
            return Ok(());
        }

        // Acquire per-media lock to prevent concurrent refreshes.
        let _guard = STREAM_LOCKS.lock(media.id).await;

        // Re-check after acquiring lock — another task may have just refreshed.
        let refreshed_at = sqlx::query_scalar::<_, Option<chrono::NaiveDateTime>>(
            "SELECT streams_refreshed_at FROM media WHERE id = ?",
        )
        .bind(media.id)
        .fetch_optional(&ctx.db)
        .await
        .ok()
        .flatten()
        .flatten();
        if is_fresh(refreshed_at) {
            return Ok(());
        }

        let instant = Instant::now();
        let raw = self.get_streams(media, ctx).await?;
        tracing::debug!(raw_count = raw.len(), "raw streams fetched");

        // Dedup by descriptor content; order preserves addon priority (DB load order).
        // First occurrence wins, so higher-priority addons' streams survive.
        let mut seen = std::collections::HashSet::new();
        let deduped: Vec<db::Media> = raw
            .into_iter()
            .filter(|s| match Self::stream_dedup_key(s) {
                Some(key) => seen.insert(key),
                None => true,
            })
            .collect();

        info!(streams = deduped.len(), elapsed = ?instant.elapsed(), "streams synced");
        if deduped.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().naive_utc();
        let sources: Vec<db::Media> = deduped
            .into_iter()
            .enumerate()
            .map(|(idx, mut s)| {
                // Stable ID derived from content so the same stream always maps to
                // the same UUID across refreshes, enabling safe upsert semantics.
                let id_key = Self::stream_dedup_key(&s)
                    .unwrap_or_else(|| format!("source_{idx}"));
                s.id = Uuid::new_v5(&media.id, id_key.as_bytes());
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
            "DELETE FROM media WHERE kind = 'stream' AND parent_id = ? AND updated_at < datetime('now', '-7 days')",
        )
        .bind(media.id)
        .execute(&ctx.db)
        .await?;
        Ok(())
    }

    pub async fn fetch_segments(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> MediaSegments {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| {
                let has_resource = r.supports(ResourceType::Segment);
                let kind_supports = r.kind.segment_supports(media);
                tracing::debug!(
                    addon = %r.row.name,
                    media_kind = ?media.kind,
                    has_resource,
                    kind_supports,
                    "segment addon filter"
                );
                has_resource && kind_supports
            })
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        let mut merged = MediaSegments::default();
        for (name, addon) in addons {
            match addon.segment_fetch(media, ctx).await {
                Ok(segs) if !segs.is_empty() => merged.merge_from(segs),
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(addon = %name, item = %media.id, error = %e, "segment addon failed")
                }
            }
        }
        merged
    }

    pub async fn lyric_fetch(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Option<LyricDto>> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| r.supports(ResourceType::Lyrics))
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        for (name, addon) in addons {
            match addon.lyric_fetch(req).await {
                Ok(Some(l)) => return Ok(Some(l)),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(addon = %name, error = %e, "lyric addon fetch failed")
                }
            }
        }
        Ok(None)
    }

    pub async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let guard = self.inner.read().await;
        let addons: Vec<(String, Arc<dyn AddonKind>)> = guard
            .iter()
            .filter(|r| r.supports(ResourceType::Lyrics))
            .map(|r| (r.row.name.clone(), r.kind.clone()))
            .collect();
        drop(guard);

        let mut out = Vec::new();
        for (name, addon) in addons {
            match addon.lyric_search(req).await {
                Ok(items) => out.extend(items),
                Err(e) => {
                    tracing::warn!(addon = %name, error = %e, "lyric addon search failed")
                }
            }
        }
        Ok(out)
    }

    pub async fn lyric_get_by_composite_id(
        &self,
        composite_id: &str,
    ) -> Result<Option<LyricDto>> {
        let guard = self.inner.read().await;
        let addons: Vec<Arc<dyn AddonKind>> = guard
            .iter()
            .filter(|r| r.supports(ResourceType::Lyrics))
            .map(|r| r.kind.clone())
            .collect();
        drop(guard);

        for addon in addons {
            if let Some(provider_id) = addon.lyric_provider_id() {
                let prefix = format!("{}_", provider_id);
                if let Some(inner) = composite_id.strip_prefix(&prefix) {
                    return addon.lyric_get_by_id(inner).await;
                }
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// AddonCatalogStream
// ---------------------------------------------------------------------------

struct AddonCatalogStream {
    addon: Arc<dyn AddonKind>,
    local_id: String,
}

#[async_trait]
impl RemoteMediaStream for AddonCatalogStream {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>> {
        self.addon
            .catalog_stream(ctx, &self.local_id)
            .await?
            .ok_or_else(|| {
                anyhow!("catalog addon does not serve catalog '{}'", self.local_id)
            })
    }
}

pub fn make_media_id(addon_id: Uuid, local_id: &str) -> String {
    format!("addon:{addon_id}:{local_id}")
}
