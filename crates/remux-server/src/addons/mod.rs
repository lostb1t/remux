//! Unified addon abstraction. Each addon kind declares which resources ×
//! media types it serves; user-added instances are rows in the `addons` table.

pub mod addon;
pub mod deezer;
pub mod eclipse;
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
use arc_swap::ArcSwap;
use async_trait::async_trait;
use futures::Stream;
use sqlx::SqlitePool;
use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::keyed_lock::KeyedLock;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{AppContext, api, common::ProgressReporter, db, sdks};
pub use addon::{Addon, CatalogState};

pub use remux_sdks::remux::AddonPresetRef;
use remux_sdks::remux::{LyricDto, MediaSegments, RemoteLyricInfoDto};

pub use remux_sdks::{
    remux::{
        AddonCatalogDto, AddonDto, AddonMetadata, AddonOption, AddonOptionType,
        AddonSelectOption, CreateAddonRequest, MediaKind, UpdateAddonCatalogRequest,
        UpdateAddonRequest,
    },
    stremio::ResourceType,
};

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
    /// The MediaKind of items this specific catalog yields.
    pub media_kind: Option<db::MediaKind>,
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
            media_kind: None,
        }
    }
}

/// A `CatalogInfo` merged with its persisted `CatalogState` override (if any) —
/// the single, fully-resolved view of a catalog that callers should use. Avoids
/// every caller re-implementing "use the stored override, else fall back to the
/// provider's declared default" itself.
#[derive(Debug, Clone)]
pub struct ResolvedCatalog {
    pub provider_catalog_id: String,
    /// Full "addon:{addon_id}:{provider_catalog_id}" id, usable with `make_catalog_stream()`.
    pub catalog_id: String,
    /// Deterministic collection id for this catalog's `media_relations` membership.
    pub collection_id: Uuid,
    pub name: String,
    pub media_kind: Option<db::MediaKind>,
    pub collection_media_kind: Option<db::CollectionMediaKind>,
    pub enabled: bool,
    pub max_items: Option<i64>,
    pub tags: Vec<String>,
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
        .filter_map(|m| {
            m.relations
                .as_ref()
        })
        .flatten()
        .filter(|(_, m)| {
            m.kind == db::MediaKind::Person
                && m.external_ids
                    .tmdb
                    .is_none()
        })
        .map(|(_, m)| m.id)
        .collect();

    // One batched upsert for all relation media (persons/genres) across the whole slice —
    // avoids opening a separate transaction per item (N items → N transactions otherwise).
    let all_rel_media: Vec<db::Media> = items
        .iter()
        .filter_map(|m| {
            m.relations
                .as_ref()
        })
        .flatten()
        .map(|(_, m)| m.clone())
        .filter(|m| !name_keyed_person_ids.contains(&m.id))
        .collect();
    if !all_rel_media.is_empty() {
        if let Err(e) = db::Media::upsert(&ctx.db, &all_rel_media).await {
            warn!(error = %e, "failed to upsert relation media batch");
        }
    }

    // Collect items that have relations, then batch delete + batch upsert
    // (replaces N×delete + N×upsert with 1 delete + 1 upsert).
    let items_with_rels: Vec<&db::Media> = items
        .iter()
        .filter(|m| {
            m.relations
                .as_ref()
                .map_or(false, |r| !r.is_empty())
        })
        .collect();
    if items_with_rels.is_empty() {
        return;
    }

    let all_ids: Vec<uuid::Uuid> = items_with_rels
        .iter()
        .map(|m| m.id)
        .collect();
    // Always use m.id as left_media_id — relations may have been built against a
    // temporary UUID (e.g. before IMDB resolution in stremio_search) that was later
    // recomputed to the stable UUID. m.id is the authoritative current identity.
    let all_rels: Vec<db::MediaRelation> = items_with_rels
        .iter()
        .flat_map(|m| {
            let current_id = m.id;
            m.relations
                .as_ref()
                .unwrap()
                .iter()
                .map(move |(r, _)| db::MediaRelation {
                    left_media_id: current_id,
                    ..r.clone()
                })
        })
        // Don't link relations that point to name-keyed person stubs.
        .filter(|r| !name_keyed_person_ids.contains(&r.right_media_id))
        .collect();
    db::MediaRelation::delete_by_left_ids(&ctx.db, &all_ids)
        .await
        .ok();
    if !all_rels.is_empty() {
        if let Err(e) = db::MediaRelation::upsert(&ctx.db, &all_rels).await {
            warn!(error = %e, "failed to upsert relations batch");
        }
    }
}

pub(crate) fn merge_media(target: &mut db::Media, source: &db::Media, replace: bool) {
    use remux_utils::merge_option;

    if (replace
        || target
            .title
            .is_empty())
        && !source
            .title
            .is_empty()
    {
        target.title = source
            .title
            .clone();
    }

    merge_option(&mut target.description, &source.description, replace);
    merge_option(&mut target.released_at, &source.released_at, replace);
    merge_option(&mut target.runtime, &source.runtime, replace);
    merge_option(
        &mut target.rating_audience,
        &source.rating_audience,
        replace,
    );
    merge_option(&mut target.certification, &source.certification, replace);
    merge_option(
        &mut target.certification_age,
        &source.certification_age,
        replace,
    );
    merge_option(&mut target.country, &source.country, replace);
    merge_option(&mut target.trailers, &source.trailers, replace);
    merge_option(
        &mut target.digital_released_at,
        &source.digital_released_at,
        replace,
    );
    merge_option(&mut target.status, &source.status, replace);
    merge_option(&mut target.idx, &source.idx, replace);
    merge_option(&mut target.parent_idx, &source.parent_idx, replace);

    target
        .external_ids
        .merge(&source.external_ids, replace);
    merge_option(
        &mut target.external_ratings,
        &source.external_ratings,
        replace,
    );
    if source
        .external_ratings
        .is_some()
    {
        merge_option(&mut target.rating_audience, &source.rating_audience, true);
    }
}

pub(crate) fn apply_title_format(media: &mut db::Media) {
    if media.kind == db::MediaKind::Season {
        media.title = format!(
            "Season {}",
            media
                .idx
                .unwrap_or(1)
        );
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
    if !patch
        .images
        .is_empty()
    {
        use remux_utils::merge_vec;
        let patch_images = std::mem::take(&mut patch.images);
        let imgs = &mut media.images;
        merge_vec(&mut imgs.primary, patch_images.primary, replace);
        merge_vec(&mut imgs.backdrop, patch_images.backdrop, replace);
        merge_vec(&mut imgs.logo, patch_images.logo, replace);
        merge_vec(&mut imgs.thumb, patch_images.thumb, replace);
    }

    merge_media(media, &patch, replace);

    if let Some(relations) = patch.relations {
        if !relations.is_empty()
            && matches!(
                media.kind,
                db::MediaKind::Movie
                    | db::MediaKind::Series
                    | db::MediaKind::Episode
                    | db::MediaKind::Album
            )
        {
            let pending: Vec<(db::MediaRelation, db::Media)> = relations
                .into_iter()
                .collect();
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
            .or_else(|| {
                si.description
                    .clone()
            })
            .unwrap_or_default();
        let probe_data = si
            .probe_data
            .clone();
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

pub(super) fn make_http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("remux-server/1.0")
        .build()
        .expect("failed to build HTTP client")
}

pub fn registered_presets() -> Vec<Box<dyn AddonPreset>> {
    inventory::iter::<AddonPresetRegistration>
        .into_iter()
        .map(|r| (r.0)())
        .collect()
}

// ---------------------------------------------------------------------------
// AddonPreset trait — kind descriptor + factory
// ---------------------------------------------------------------------------

pub trait AddonPreset: Send + Sync {
    fn id(&self) -> &'static str;
    fn metadata(&self) -> AddonMetadata;
    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        config: &crate::Config,
    ) -> Result<AddonCapabilities>;

    /// Transform the config before it is persisted to the DB.
    /// Use this to convert inline secrets into file references, strip write-only fields, etc.
    /// The default is a no-op.
    fn normalize_cfg(
        &self,
        cfg: serde_json::Value,
        _config: &crate::Config,
    ) -> Result<serde_json::Value> {
        Ok(cfg)
    }
}

// ---------------------------------------------------------------------------
// AddonKind — lean identity + manifest trait
// ---------------------------------------------------------------------------

#[async_trait]
pub trait AddonKind: Send + Sync {
    fn id(&self) -> &'static str;

    /// Returns `Ok(Some((resources, types)))` when the addon can determine its
    /// own capabilities (e.g. by fetching a remote manifest).
    /// Returns `Ok(None)` to signal "no override — caller should fall back to
    /// the preset's `metadata().supported_*`".
    /// Returns `Err` when a required remote fetch fails and the addon cannot
    /// be used (the error propagates to the API caller).
    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<remux_sdks::stremio::ResourceRef>,
            Vec<remux_sdks::stremio::MediaType>,
        )>,
    > {
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// Capability traits
// ---------------------------------------------------------------------------

#[async_trait]
pub trait IndexAddon: Send + Sync {
    async fn refresh_index(
        &self,
        ctx: &AppContext,
        addon: &Addon,
        progress: ProgressReporter,
    ) -> Result<()>;
    async fn purge_index(&self, ctx: &AppContext, addon: &Addon) -> Result<()>;
}

#[async_trait]
pub trait CatalogAddon: Send + Sync {
    async fn catalog_list(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>>;
    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>>;
}

#[async_trait]
pub trait MetaAddon: Send + Sync {
    async fn supports(&self, media: &db::Media) -> bool;
    /// Fetch metadata for `media` and return a partial `db::Media` patch.
    /// Only the fields the addon knows about need to be populated; the caller
    /// merges the patch into the existing record via `merge_media`.
    /// Populate `patch.images` for images, `patch.relations` for people/genres.
    async fn meta_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        config: &api::ServerConfiguration,
    ) -> Result<Option<db::Media>>;
    /// Fetch remote image candidates for manual image selection in the UI.
    async fn images_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        Ok(vec![])
    }
}

#[async_trait]
pub trait TreeAddon: Send + Sync {
    fn supports(&self, root: &db::Media) -> bool;
    async fn get_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
}

#[async_trait]
pub trait SearchAddon: Send + Sync {
    async fn search_supports(&self, kind: &db::MediaKind) -> bool;
    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
}

#[derive(Clone)]
pub struct SubtitleInfo {
    pub id: String,
    pub url: Option<crate::stream::StreamDescriptor>,
    pub lang: Option<String>,
    pub is_forced: bool,
    pub is_hi: bool,
}

#[async_trait]
pub trait SubtitleAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Result<Vec<SubtitleInfo>>;
}

#[async_trait]
pub trait StreamAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>>;
    /// Serve bytes for a stream that requires this addon's config (e.g. credentials).
    /// Only called when `StreamDescriptor::addon_id()` points to this addon.
    async fn serve_stream(
        &self,
        descriptor: &crate::stream::StreamDescriptor,
        headers: &axum::http::HeaderMap,
    ) -> axum_anyhow::ApiResult<axum::response::Response> {
        Err(axum_anyhow::ApiError::builder()
            .status(axum::http::StatusCode::BAD_REQUEST)
            .title("stream")
            .detail("serve_stream not implemented for this addon")
            .build())
    }
}

#[async_trait]
pub trait SegmentAddon: Send + Sync {
    fn supports(&self, media: &db::Media) -> bool;
    async fn segment_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<MediaSegments>;
}

#[async_trait]
pub trait LyricAddon: Send + Sync {
    fn provider_id(&self) -> String;
    async fn lyric_fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>>;
    async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>>;
    async fn lyric_get_by_id(&self, id: &str) -> Result<Option<LyricDto>>;
}

// ---------------------------------------------------------------------------
// AddonCapabilities — produced by AddonPreset::from_cfg
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct AddonCapabilities {
    pub metadata: AddonMetadata,
    pub kind: Option<Arc<dyn AddonKind>>,
    pub catalog: Option<Arc<dyn CatalogAddon>>,
    pub meta: Option<Arc<dyn MetaAddon>>,
    pub stream: Option<Arc<dyn StreamAddon>>,
    pub search: Option<Arc<dyn SearchAddon>>,
    pub subtitle: Option<Arc<dyn SubtitleAddon>>,
    pub tree: Option<Arc<dyn TreeAddon>>,
    pub segment: Option<Arc<dyn SegmentAddon>>,
    pub lyric: Option<Arc<dyn LyricAddon>>,
    pub index: Option<Arc<dyn IndexAddon>>,
}

// ---------------------------------------------------------------------------
// AddonRuntime — one entry in the service Vec
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct AddonRuntime {
    pub row: Addon,
    pub caps: AddonCapabilities,
}

impl std::ops::Deref for AddonRuntime {
    type Target = AddonCapabilities;
    fn deref(&self) -> &Self::Target {
        &self.caps
    }
}

impl AddonRuntime {
    /// Fetches this addon's live catalog list and merges in its persisted
    /// per-catalog overrides (enabled/max_items/tags). Catalogs without a
    /// stored override fall back to the provider's own declared defaults.
    pub async fn resolve_catalogs(
        &self,
        ctx: &AppContext,
    ) -> Result<Vec<ResolvedCatalog>> {
        let Some(catalog) = self
            .catalog
            .as_ref()
        else {
            return Ok(vec![]);
        };
        let available = catalog
            .catalog_list(ctx)
            .await?;
        let states = self
            .row
            .catalog_states();
        let addon_id = self
            .row
            .id;
        Ok(available
            .into_iter()
            .map(|info| {
                let state = states.get(&info.provider_catalog_id);
                ResolvedCatalog {
                    catalog_id: make_media_id(addon_id, &info.provider_catalog_id),
                    collection_id: Uuid::new_v5(
                        &addon_id,
                        info.provider_catalog_id
                            .as_bytes(),
                    ),
                    enabled: state
                        .map(|s| s.enabled)
                        .unwrap_or(info.default_enabled),
                    max_items: state
                        .and_then(|s| s.max_items)
                        .or(info.default_max_items),
                    tags: state
                        .map(|s| {
                            s.tags
                                .clone()
                        })
                        .unwrap_or_default(),
                    provider_catalog_id: info.provider_catalog_id,
                    name: info.name,
                    media_kind: info.media_kind,
                    collection_media_kind: info.collection_media_kind,
                }
            })
            .collect())
    }

    pub fn supports_type(&self, kind: &db::MediaKind) -> bool {
        // Manifest types (live metadata) are the authoritative upper bound.
        // "Series" in a type list covers Episode and Season too (Stremio model).
        let mt: Vec<db::MediaKind> = self
            .caps
            .metadata
            .supported_types
            .iter()
            .cloned()
            .map(db::MediaKind::from)
            .collect();
        if !mt.is_empty() && !kind_in_type_list(kind, &mt) {
            return false;
        }
        self.row
            .types
            .is_empty()
            || kind_in_type_list(
                kind,
                &self
                    .row
                    .types,
            )
    }

    /// Returns the `idPrefixes` declared for a resource in the live manifest metadata,
    /// or `None` if the resource has no prefix restriction.
    fn resource_id_prefixes(&self, kind: &ResourceType) -> Option<&[String]> {
        self.caps
            .metadata
            .supported_resources
            .iter()
            .find(|r| &r.name == kind)
            .and_then(|r| {
                r.id_prefixes
                    .as_deref()
            })
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
    inner: Arc<ArcSwap<Vec<AddonRuntime>>>,
}

#[async_trait]
trait PickCap<T: ?Sized + Send + Sync> {
    async fn pick(&self, media: &db::Media) -> bool;
}

#[async_trait]
impl PickCap<dyn MetaAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        let Some(cap) = self
            .caps
            .meta
            .as_ref()
        else {
            return false;
        };
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Meta) {
            let Some(id) = media
                .external_ids
                .stremio_lookup_id()
            else {
                return false;
            };
            return prefixes
                .iter()
                .any(|p| id.starts_with(p.as_str()));
        }
        cap.supports(media)
            .await
    }
}

#[async_trait]
impl PickCap<dyn StreamAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        if !self
            .row
            .resources
            .contains(&ResourceType::Stream)
        {
            return false;
        }
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Stream) {
            let Some(id) = media
                .external_ids
                .stremio_lookup_id()
            else {
                return false;
            };
            return prefixes
                .iter()
                .any(|p| id.starts_with(p.as_str()));
        }
        match self
            .caps
            .stream
            .as_ref()
        {
            Some(cap) => cap.supports(media),
            None => false,
        }
    }
}

#[async_trait]
impl PickCap<dyn SubtitleAddon> for AddonRuntime {
    async fn pick(&self, media: &db::Media) -> bool {
        if !self
            .row
            .resources
            .contains(&ResourceType::Subtitles)
        {
            return false;
        }
        if let Some(prefixes) = self.resource_id_prefixes(&ResourceType::Subtitles) {
            let Some(id) = media
                .external_ids
                .stremio_lookup_id()
            else {
                return false;
            };
            return prefixes
                .iter()
                .any(|p| id.starts_with(p.as_str()));
        }
        match self
            .caps
            .subtitle
            .as_ref()
        {
            Some(cap) => cap.supports(media),
            None => false,
        }
    }
}

impl AddonService {
    async fn addons_for<T>(&self, media: &db::Media) -> Vec<AddonRuntime>
    where
        T: ?Sized + Send + Sync + 'static,
        AddonRuntime: PickCap<T>,
    {
        let mut out = Vec::new();
        for r in self
            .inner
            .load()
            .iter()
            .filter(|r| r.supports_type(&media.kind))
        {
            if PickCap::<T>::pick(r, media).await {
                out.push(r.clone());
            }
        }
        out
    }

    pub async fn from_db(db: &SqlitePool, config: &crate::Config) -> Result<Self> {
        let runtimes = Self::load_runtimes(db, config).await?;
        Ok(Self {
            inner: Arc::new(ArcSwap::from_pointee(runtimes)),
        })
    }

    async fn load_runtimes(
        db: &SqlitePool,
        config: &crate::Config,
    ) -> Result<Vec<AddonRuntime>> {
        let presets = registered_presets();
        let addons = Addon::list(db).await?;
        let mut runtimes = Vec::new();

        for mut addon in addons
            .into_iter()
            .filter(|a| a.enabled)
        {
            let Some(preset) = presets
                .iter()
                .find(|p| {
                    p.id()
                        == addon
                            .preset
                            .kind
                })
            else {
                warn!(
                    addon_id = %addon.id,
                    kind = %addon.preset.kind,
                    "skipping addon with unknown preset kind"
                );
                continue;
            };
            match preset.from_cfg(
                addon.id,
                &addon
                    .preset
                    .config,
                config,
            ) {
                Ok(mut caps) => {
                    // Start with the preset's static metadata, then upgrade with live manifest data.
                    caps.metadata = preset.metadata();
                    if let Some(ref kind) = caps.kind {
                        match kind
                            .available_info()
                            .await
                        {
                            Ok(Some((resource_refs, raw_types))) => {
                                caps.metadata
                                    .supported_resources = resource_refs;
                                if !raw_types.is_empty() {
                                    caps.metadata
                                        .supported_types = raw_types
                                        .into_iter()
                                        .map(Into::into)
                                        .collect();
                                }
                            }
                            Ok(None) => {}
                            Err(e) => {
                                warn!(
                                    addon_id = %addon.id,
                                    name = %addon.name,
                                    error = %e,
                                    "failed to fetch addon manifest at load time"
                                );
                            }
                        }
                    }
                    runtimes.push(AddonRuntime { row: addon, caps });
                }
                Err(e) => warn!(
                    addon_id = %addon.id,
                    kind = %addon.preset.kind,
                    error = %e,
                    "failed to instantiate addon"
                ),
            }
        }
        Ok(runtimes)
    }

    pub async fn reload(&self, db: &SqlitePool, config: &crate::Config) -> Result<()> {
        let runtimes = Self::load_runtimes(db, config).await?;
        self.inner
            .store(Arc::new(runtimes));
        Ok(())
    }

    pub fn list(&self) -> arc_swap::Guard<Arc<Vec<AddonRuntime>>> {
        self.inner
            .load()
    }

    pub fn get(&self, id: Uuid) -> Option<AddonRuntime> {
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .cloned()
    }

    pub fn catalog_addons(&self) -> Vec<AddonRuntime> {
        self.inner
            .load()
            .iter()
            .filter(|r| {
                r.catalog
                    .is_some()
            })
            .cloned()
            .collect()
    }

    /// Returns `(addon, catalogs)` pairs for every catalog-capable addon that could
    /// produce any of `kinds`, with each addon's catalog list already filtered down to
    /// catalogs whose own `media_kind` is one of `kinds`. Addons are pre-filtered via
    /// `supports_type` as a cheap upper-bound check before calling `catalog_list()`
    /// (which may hit the network); per-addon listing errors are logged and skipped.
    pub async fn catalogs_for_kinds(
        &self,
        ctx: &AppContext,
        kinds: &[db::MediaKind],
    ) -> Vec<(AddonRuntime, Vec<ResolvedCatalog>)> {
        let mut out = Vec::new();
        for runtime in self
            .catalog_addons()
            .into_iter()
            .filter(|r| {
                kinds
                    .iter()
                    .any(|k| r.supports_type(k))
            })
        {
            let addon_id = runtime
                .row
                .id;
            let resolved = match runtime
                .resolve_catalogs(ctx)
                .await
            {
                Ok(v) => v
                    .into_iter()
                    .filter(|c| {
                        c.media_kind
                            .as_ref()
                            .is_some_and(|mk| kinds.contains(mk))
                    })
                    .collect(),
                Err(e) => {
                    warn!(addon = %addon_id, error = %e, "failed to list addon catalogs, skipping");
                    continue;
                }
            };
            out.push((runtime, resolved));
        }
        out
    }

    pub async fn purge_indexes(&self, ctx: &AppContext) -> Result<()> {
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .cloned()
            .collect();
        for runtime in &addons {
            if let Some(index) = &runtime.index {
                if let Err(e) = index
                    .purge_index(ctx, &runtime.row)
                    .await
                {
                    warn!(addon = %runtime.row.name, error = %e, "purge_index failed");
                }
            }
        }
        Ok(())
    }

    pub async fn refresh_indexes(
        &self,
        ctx: &AppContext,
        progress: ProgressReporter,
    ) -> Result<()> {
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .enabled
            })
            .cloned()
            .collect();
        let total = addons.len();
        for (idx, runtime) in addons
            .iter()
            .enumerate()
        {
            if let Some(index) = &runtime.index {
                let sub = progress.step(idx, total);
                if let Err(e) = index
                    .refresh_index(ctx, &runtime.row, sub)
                    .await
                {
                    warn!(addon = %runtime.row.name, error = %e, "refresh_index failed");
                }
            }
        }
        progress.set(100.0);
        Ok(())
    }

    pub fn get_catalog(&self, id: Uuid) -> Option<Arc<dyn CatalogAddon>> {
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .and_then(|r| {
                r.catalog
                    .clone()
            })
    }

    /// Return the tags configured for a specific catalog within an addon.
    pub fn catalog_tags(&self, addon_uuid: &str, local_cat_id: &str) -> Vec<String> {
        let Ok(id) = Uuid::parse_str(addon_uuid) else {
            return vec![];
        };
        self.inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .map(|r| {
                r.row
                    .catalog_states()
                    .get(local_cat_id)
                    .map(|s| {
                        s.tags
                            .clone()
                    })
                    .unwrap_or_default()
            })
            .unwrap_or_default()
    }

    pub fn make_catalog_stream(
        &self,
        media_id: &str,
    ) -> Option<Box<dyn RemoteMediaStream>> {
        let rest = media_id.strip_prefix("addon:")?;
        let (uuid_str, local_id) = rest.split_once(':')?;
        let id = Uuid::parse_str(uuid_str).ok()?;
        let addon = self
            .inner
            .load()
            .iter()
            .find(|r| {
                r.row
                    .id
                    == id
            })
            .and_then(|r| {
                r.catalog
                    .clone()
            })?;
        Some(Box::new(AddonCatalogStream {
            addon,
            local_id: local_id.to_string(),
        }))
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn refresh_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
        config: &api::ServerConfiguration,
    ) -> Result<()> {
        let applicable = self
            .addons_for::<dyn MetaAddon>(media)
            .await;

        if applicable.is_empty() {
            return Ok(());
        }

        let results = futures::future::join_all(
            applicable
                .iter()
                .map(|r| {
                    r.meta
                        .as_ref()
                        .unwrap()
                        .meta_fetch(media, ctx, config)
                }),
        )
        .await;

        for (r, result) in applicable
            .iter()
            .zip(results)
        {
            match result {
                Ok(Some(patch)) => apply_meta(media, patch, force_refresh),
                Ok(None) => {}
                Err(e) => {
                    error!(addon = %r.row.name, error = %e, "meta addon error")
                }
            }
        }

        // Apply SxxExx / "Season N" title formatting once, after all patches are merged.
        // Calling it inside apply_meta would re-apply the prefix on every patch.
        apply_title_format(media);

        // Recompute stable UUID for Person once TMDB ID is resolved.
        if media.kind == db::MediaKind::Person {
            if let Some(tmdb_id) = media
                .external_ids
                .tmdb
            {
                media.id = crate::common::stable_media_uuid(
                    &db::MediaKind::Person,
                    &tmdb_id.to_string(),
                );
            }
        }

        media.refreshed_at = Some(chrono::Utc::now().naive_utc());
        Ok(())
    }

    pub async fn get_tree(&self, root: &db::Media, ctx: &AppContext) -> Vec<db::Media> {
        let mut seen: std::collections::HashSet<uuid::Uuid> =
            std::collections::HashSet::new();
        seen.insert(root.id);
        let mut all = Vec::new();
        let mut queue = vec![root.clone()];

        while let Some(node) = queue.pop() {
            let applicable: Vec<Arc<dyn TreeAddon>> = self
                .inner
                .load()
                .iter()
                .filter_map(|r| {
                    if !r
                        .tree
                        .as_ref()
                        .map(|t| t.supports(&node))
                        .unwrap_or(false)
                    {
                        return None;
                    }
                    // Apply the same meta idPrefixes guard used for MetaAddon.
                    if let Some(prefixes) = r.resource_id_prefixes(&ResourceType::Meta)
                    {
                        let Some(id) = node
                            .external_ids
                            .stremio_lookup_id()
                        else {
                            return None;
                        };
                        if !prefixes
                            .iter()
                            .any(|p| id.starts_with(p.as_str()))
                        {
                            return None;
                        }
                    }
                    r.tree
                        .as_ref()
                        .cloned()
                })
                .collect();

            for addon in &applicable {
                match addon
                    .get_children(&node, ctx)
                    .await
                {
                    Ok(Some(children)) if !children.is_empty() => {
                        for mut child in children {
                            // dbg!(&child.title);
                            //child.parent_id = Some(node.id);
                            //if node.parent_id.is_some() {
                            //  child.grandparent_id = node.parent_id;
                            //}
                            if seen.insert(child.id) {
                                queue.push(child.clone());
                                all.push(child);
                            }
                        }
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => {
                        warn!(id = %node.id, error = %e, "get_children failed");
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

        let config = db::Settings::get_config(&ctx.db)
            .await
            .unwrap_or_default();

        let concurrency = config.meta_concurrency as usize;
        let config = Arc::new(config);

        let mut stream = stream::iter(media)
            .map(|m| {
                let cfg = Arc::clone(&config);
                self.process_meta_item(m, ctx, force_refresh, cfg)
            })
            .buffer_unordered(concurrency);

        let mut batch = vec![];

        while let Some(items) = stream
            .next()
            .await
        {
            if items.is_empty() {
                continue;
            }

            batch.extend(items);

            while batch.len() >= db::CHUNK_SIZE {
                let items: Vec<_> = batch
                    .drain(..db::CHUNK_SIZE)
                    .collect();

                match db::Media::upsert(&ctx.db, &items).await {
                    Ok(_) => {
                        save_pending_relations(ctx, &items).await;
                    }
                    Err(e) => {
                        error!(error = %e, "failed to upsert media batch");
                    }
                }
            }
        }

        if !batch.is_empty() {
            match db::Media::upsert(&ctx.db, &batch).await {
                Ok(_) => {
                    save_pending_relations(ctx, &batch).await;
                }
                Err(e) => {
                    error!(error = %e, "failed to upsert final media batch");
                }
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
            warn!(id = %media.id, error = %e, "failed to refresh metadata, keeping as-is");
            return vec![media];
        }

        // If this Person's ID was rewritten (name-keyed → tmdb-keyed) by refresh_meta,
        // delete the stale name-keyed row so it doesn't linger as a duplicate.
        if media.kind == db::MediaKind::Person && media.id != original_id {
            if let Err(e) = db::Media::delete(&ctx.db, &original_id).await {
                warn!(
                    old_id = %original_id,
                    new_id = %media.id,
                    error = %e,
                    "failed to delete stale name-keyed person row"
                );
            }
        }

        let mut tree = self
            .get_tree(&media, ctx)
            .await;
        if tree.is_empty() {
            if media.kind == db::MediaKind::Series {
                info!(id = %media.id, title = %media.title, "series has no seasons yet, skipping import");
                return vec![];
            }
            return vec![media];
        }

        for item in &mut tree {
            if force_refresh
                || item
                    .refreshed_at
                    .is_none()
            {
                if let Err(e) = self
                    .refresh_meta(item, ctx, force_refresh, &config)
                    .await
                {
                    warn!(id = %item.id, error = %e, "failed to refresh child meta");
                }
            }
        }

        tree.insert(0, media);

        tree
    }

    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let addons: Vec<AddonRuntime> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.supports_type(kind)
                    && r.row
                        .resources
                        .contains(&ResourceType::Search)
                    && r.search
                        .is_some()
            })
            .cloned()
            .collect();

        for r in addons {
            if !r
                .search
                .as_ref()
                .unwrap()
                .search_supports(kind)
                .await
            {
                continue;
            }
            match r
                .search
                .as_ref()
                .unwrap()
                .search(kind, query, limit, ctx)
                .await
            {
                Ok(Some(results)) => {
                    for m in &results {
                        ctx.store
                            .save(
                                m.id.to_string(),
                                m.clone(),
                                Duration::from_secs(3600),
                            );
                    }
                    return Ok(results);
                }
                Ok(None) => continue,
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "search addon error")
                }
            }
        }
        Ok(vec![])
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn fetch_images(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::api::RemoteImageInfo>> {
        let addons = self
            .addons_for::<dyn MetaAddon>(media)
            .await;

        let mut out = Vec::new();
        for r in addons {
            match r
                .meta
                .as_ref()
                .unwrap()
                .images_fetch(media, ctx)
                .await
            {
                Ok(images) => out.extend(images),
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "images_fetch failed")
                }
            }
        }
        Ok(out)
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn fetch_subtitles(
        &self,
        media: &db::Media,
        db: &SqlitePool,
        background: bool,
    ) -> Vec<SubtitleInfo> {
        let addons = self
            .addons_for::<dyn SubtitleAddon>(media)
            .await;

        debug!(count = addons.len(), "subtitle addons matched");
        let instant = Instant::now();
        let mut subs = vec![];
        for r in &addons {
            debug!(addon = %r.row.name, "fetching subtitles from addon");
            match r
                .subtitle
                .as_ref()
                .unwrap()
                .subtitle_fetch(media, db)
                .await
            {
                Ok(s) => {
                    debug!(addon = %r.row.name, count = s.len(), "subtitle addon returned results");
                    subs.extend(s);
                }
                Err(e) => {
                    warn!(addon = %r.row.name, error = %e, "subtitle addon failed")
                }
            }
        }
        if background {
            debug!(subs = subs.len(), addons = addons.len(), elapsed = ?instant.elapsed(), "subtitles fetched");
        } else {
            info!(subs = subs.len(), addons = addons.len(), elapsed = ?instant.elapsed(), "subtitles fetched");
        }
        subs
    }

    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let addons = self
            .addons_for::<dyn StreamAddon>(media)
            .await;

        debug!(
            media_id = %media.id,
            media_kind = ?media.kind,
            addon_count = addons.len(),
            "resolving streams"
        );

        let tasks: Vec<_> = addons
            .into_iter()
            .map(|r| async move {
                let name = &r.row.name;
                match r.stream.as_ref().unwrap().get_streams(media, ctx).await {
                    Ok(mut streams) => {
                        if streams.is_empty() {
                            debug!(addon = %name, "addon: no streams");
                        } else {
                            debug!(addon = %name, count = streams.len(), "addon: streams found");
                            for s in &mut streams {
                                s.source = Some(name.clone());
                            }
                        }
                        streams
                    }
                    Err(e) => {
                        warn!(addon = %name, error = %e, "stream addon failed");
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
        match &s
            .stream_info
            .as_ref()?
            .descriptor
        {
            crate::stream::StreamDescriptor::Torrent { info_hash, .. } => {
                Some(format!("torrent:{}", info_hash.to_lowercase()))
            }
            crate::stream::StreamDescriptor::Http { url, .. } => {
                let stable = url
                    .split('?')
                    .next()
                    .unwrap_or(url.as_str());
                Some(format!("http:{stable}"))
            }
            crate::stream::StreamDescriptor::Local(path) => {
                Some(format!("local:{}", path.display()))
            }
            crate::stream::StreamDescriptor::Rtsp { url } => {
                Some(format!("rtsp:{url}"))
            }
            crate::stream::StreamDescriptor::Opendal { addon_id, path } => {
                Some(format!("opendal:{addon_id}:{path}"))
            }
        }
    }

    #[tracing::instrument(skip_all, fields(title = %media.title, kind = %media.kind))]
    pub async fn refresh_streams(
        &self,
        media: &mut db::Media,
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
        let _guard = STREAM_LOCKS
            .lock(media.id)
            .await;

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
            media.streams_refreshed_at = refreshed_at;
            return Ok(());
        }

        let instant = Instant::now();
        let raw = self
            .get_streams(media, ctx)
            .await?;
        debug!(raw_count = raw.len(), "raw streams fetched");

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

        let sources: Vec<&str> = {
            let mut seen = std::collections::HashSet::new();
            deduped
                .iter()
                .filter_map(|m| {
                    m.stream_info
                        .as_ref()?
                        .source
                        .as_deref()
                })
                .filter(|s| seen.insert(*s))
                .collect()
        };
        info!(streams = deduped.len(), ?sources, elapsed = ?instant.elapsed(), "streams synced");
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
        media.streams_refreshed_at = Some(now);

        // delete stale items
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
        background: bool,
    ) -> MediaSegments {
        let addons: Vec<(String, Arc<dyn SegmentAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Segment)
            })
            .filter_map(|r| {
                r.segment
                    .as_ref()
                    .and_then(|s| {
                        let supports = s.supports(media);
                        debug!(
                            addon = %r.row.name,
                            media_kind = ?media.kind,
                            supports,
                            "segment addon filter"
                        );
                        if supports {
                            Some((
                                r.row
                                    .name
                                    .clone(),
                                s.clone(),
                            ))
                        } else {
                            None
                        }
                    })
            })
            .collect();

        let addon_count = addons.len();
        let instant = Instant::now();
        let mut merged = MediaSegments::default();
        for (name, addon) in addons {
            match addon
                .segment_fetch(media, ctx)
                .await
            {
                Ok(segs) if !segs.is_empty() => merged.merge_from(segs),
                Ok(_) => {}
                Err(e) => {
                    error!(addon = %name, item = %media.id, error = %e, "segment addon failed")
                }
            }
        }
        let found = [
            &merged.intro,
            &merged.outro,
            &merged.recap,
            &merged.preview,
            &merged.commercial,
        ]
        .iter()
        .filter(|s| s.is_some())
        .count();
        if background {
            debug!(segments = found, addons = addon_count, elapsed = ?instant.elapsed(), "segments fetched");
        } else {
            info!(segments = found, addons = addon_count, elapsed = ?instant.elapsed(), "segments fetched");
        }
        merged
    }

    pub async fn lyric_fetch(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Option<LyricDto>> {
        let addons: Vec<(String, Arc<dyn LyricAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Lyrics)
            })
            .filter_map(|r| {
                r.lyric
                    .as_ref()
                    .map(|l| {
                        (
                            r.row
                                .name
                                .clone(),
                            l.clone(),
                        )
                    })
            })
            .collect();

        for (name, addon) in addons {
            match addon
                .lyric_fetch(req)
                .await
            {
                Ok(Some(l)) => return Ok(Some(l)),
                Ok(None) => continue,
                Err(e) => {
                    warn!(addon = %name, error = %e, "lyric addon fetch failed")
                }
            }
        }
        Ok(None)
    }

    pub async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let addons: Vec<(String, Arc<dyn LyricAddon>)> = self
            .inner
            .load()
            .iter()
            .filter(|r| {
                r.row
                    .resources
                    .contains(&ResourceType::Lyrics)
            })
            .filter_map(|r| {
                r.lyric
                    .as_ref()
                    .map(|l| {
                        (
                            r.row
                                .name
                                .clone(),
                            l.clone(),
                        )
                    })
            })
            .collect();

        let mut out = Vec::new();
        for (name, addon) in addons {
            match addon
                .lyric_search(req)
                .await
            {
                Ok(items) => out.extend(items),
                Err(e) => {
                    warn!(addon = %name, error = %e, "lyric addon search failed")
                }
            }
        }
        Ok(out)
    }

    pub async fn lyric_get_by_composite_id(
        &self,
        composite_id: &str,
    ) -> Result<Option<LyricDto>> {
        let addons: Vec<Arc<dyn LyricAddon>> = self
            .inner
            .load()
            .iter()
            .filter_map(|r| {
                r.lyric
                    .clone()
            })
            .collect();

        for addon in addons {
            let prefix = format!("{}_", addon.provider_id());
            if let Some(inner) = composite_id.strip_prefix(&prefix) {
                return addon
                    .lyric_get_by_id(inner)
                    .await;
            }
        }
        Ok(None)
    }
}

// ---------------------------------------------------------------------------
// AddonCatalogStream
// ---------------------------------------------------------------------------

struct AddonCatalogStream {
    addon: Arc<dyn CatalogAddon>,
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
