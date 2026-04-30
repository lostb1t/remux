//! Unified addon abstraction. Each addon kind declares which resources ×
//! media types it serves; user-added instances are rows in the `addons` table.

pub mod deezer;
pub mod introdb;
pub mod lrclib;
pub mod probe;
pub mod row;
pub mod squid;
pub mod stremio;
pub mod tmdb;
pub mod ytdlp;

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use futures::Stream;
use sqlx::SqlitePool;
use std::pin::Pin;
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

pub use crate::db::{MetaRelation, MetaResult};
use crate::sdks;
use crate::{AppContext, db};
use remux_sdks::remux::models::{LyricDto, MediaSegments, RemoteLyricInfoDto};

pub use remux_sdks::remux::addons::{
    AddonCatalogDto, AddonDto, AddonKindMetadata, AddonOption, AddonOptionType,
    AddonResource, AddonSelectOption, CreateAddonRequest, UpdateAddonCatalogRequest,
    UpdateAddonRequest,
};
pub use row::{AddonRow, CatalogState};

/// Provider-agnostic descriptor for a single remote catalog.
#[derive(Debug, Clone)]
pub struct CatalogInfo {
    pub provider_catalog_id: String,
    pub name: String,
}

/// A configured remote source that streams `db::Media` items for upsert.
#[async_trait]
pub trait RemoteMediaStream: Send + Sync {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>>;
}

/// Cached music search result stored in `AppContext::store`.
#[derive(Clone)]
pub struct MusicSearchResult {
    pub media: db::Media,
    pub album: Option<db::Media>,
    pub artist: Option<db::Media>,
}

#[derive(Debug)]
pub struct LyricSearchRequest {
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

/// Merge fields from `source` into `target`. If `replace` is true, overwrites
/// existing values; otherwise only fills `None`/empty fields.
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
    macro_rules! fill_image {
        ($field:ident) => {
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

/// Compile-time registration of an addon kind. Uses `inventory` to mirror
/// the existing route registration pattern (`#[get(...)]` etc.).
pub struct AddonKindRegistration(pub fn() -> Box<dyn AddonKind>);
inventory::collect!(AddonKindRegistration);

/// Static descriptor for one kind of addon (e.g. "stremio", "deezer").
/// One per kind, registered via `inventory::submit!`.
pub trait AddonKind: Send + Sync {
    /// Stable identifier — matches the `addons.kind` column. Lowercase, no spaces.
    fn id(&self) -> &'static str;

    /// Form schema, supported resources & media types. Returned via
    /// `GET /addon-kinds` so the dashboard can render the form generically.
    fn metadata(&self) -> AddonKindMetadata;

    /// Build a runtime addon from a stored row. The row's `config` JSON has
    /// already been validated against this kind's option schema by the API layer.
    fn instantiate(&self, row: &AddonRow) -> Result<Box<dyn Addon>>;
}

/// Runtime instance of an addon. Built on demand by the registry from a
/// stored row. Every resource method defaults to a "this addon doesn't
/// serve that resource" return so kinds only implement what they actually
/// serve. The registry enforces that an addon only sees calls for resources
/// listed in its `row().resources`, so individual implementations don't
/// need to re-check enablement.
#[async_trait]
pub trait Addon: Send + Sync {
    /// The DB row this instance was built from.
    fn row(&self) -> &AddonRow;

    /// Resources this addon instance actually provides. Defaults to the
    /// enabled resources saved in the row. Override to provide dynamic values
    /// (e.g. Stremio fetches from its manifest).
    async fn supported_resources(&self) -> Vec<AddonResource> {
        self.row().resources.clone()
    }

    /// Content types this addon instance supports (e.g. `"movie"`, `"series"`).
    /// Defaults to the static list from the kind's metadata. Override to
    /// provide dynamic values (e.g. Stremio fetches from its manifest).
    async fn supported_types(&self) -> Vec<String> {
        registered_kinds()
            .into_iter()
            .find(|k| k.id() == self.row().kind)
            .map(|k| k.metadata().supported_types)
            .unwrap_or_default()
    }

    // --- Catalog ---

    /// Stream items from this addon's catalog identified by `local_id`.
    /// `None` = unknown local_id or addon doesn't serve catalogs.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, local_id = %_local_id), err)]
    async fn catalog_stream(
        &self,
        _ctx: &AppContext,
        _local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        Ok(None)
    }

    /// List catalogs this addon currently exposes.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name), err)]
    async fn list_catalogs(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![])
    }

    // --- Meta ---

    /// Whether this addon can fill metadata for `media`.
    async fn meta_supports(&self, _media: &db::Media) -> bool {
        false
    }

    /// Fetch metadata for `media`. `None` = doesn't apply / not found.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, media_id = %_media.id, kind = ?_media.kind), err)]
    async fn meta(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<MetaResult>> {
        Ok(None)
    }

    /// Refresh metadata across an already-synced tree (e.g. set season/episode
    /// titles after children are persisted). Optional; default no-op.
    #[tracing::instrument(skip(self, _children, _ctx), fields(addon = %self.row().name, root_id = %_root.id), err)]
    async fn refresh_tree_meta(
        &self,
        _root: &db::Media,
        _children: &mut [db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }

    // --- Hierarchy sync (was HierarchySyncProvider) ---

    /// Whether this addon can discover children for a root media kind.
    fn hierarchy_supports(&self, _root: &db::Media) -> bool {
        false
    }

    /// Discover children under a root media item. `None` if this addon
    /// doesn't sync children for this root kind.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, root_id = %_root.id), err)]
    async fn sync_children(
        &self,
        _root: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        Ok(None)
    }

    /// Hook to persist additional metadata about discovered children
    /// (e.g. season images). Optional.
    #[tracing::instrument(skip(self, _children, _ctx), fields(addon = %self.row().name, root_id = %_root.id), err)]
    async fn persist_children_metadata(
        &self,
        _root: &db::Media,
        _children: &[db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }

    // --- Search ---

    /// Whether this addon can search for media of `kind`.
    async fn search_supports(&self, _kind: &db::MediaKind) -> bool {
        false
    }

    /// Search and cache results. Returning `None` means "not applicable";
    /// the registry tries the next addon.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, kind = ?_kind, query = %_query), err)]
    async fn search(
        &self,
        _kind: &db::MediaKind,
        _query: &str,
        _limit: usize,
        _ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        Ok(None)
    }

    /// Persist a search-cached item to the DB. Returns the persisted media
    /// if this addon owns `id`, otherwise `None`.
    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, id = %_id), err)]
    async fn persist_search_result(
        &self,
        _id: Uuid,
        _ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        Ok(None)
    }

    // --- Subtitles ---

    /// Whether this addon can fetch subtitles for `media`.
    fn subtitles_supports(&self, _media: &db::Media) -> bool {
        false
    }

    #[tracing::instrument(skip(self, _db), fields(addon = %self.row().name, media_id = %_media.id), err)]
    async fn subtitles(
        &self,
        _media: &db::Media,
        _db: &SqlitePool,
    ) -> Result<Vec<sdks::stremio::Subtitle>> {
        Ok(vec![])
    }

    // --- Streams ---

    /// Whether this addon can resolve playable streams for `media`.
    fn streams_supports(&self, _media: &db::Media) -> bool {
        false
    }

    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, media_id = %_media.id), err)]
    async fn streams(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        Ok(vec![])
    }

    // --- Segments ---

    /// Whether this addon can fetch timeline segments for `media`.
    fn segments_supports(&self, _media: &db::Media) -> bool {
        false
    }

    #[tracing::instrument(skip(self, _ctx), fields(addon = %self.row().name, media_id = %_media.id), err)]
    async fn segments(
        &self,
        _media: &db::Media,
        _ctx: &AppContext,
    ) -> Result<MediaSegments> {
        Ok(MediaSegments::default())
    }

    // --- Lyrics ---

    #[tracing::instrument(skip(self), fields(addon = %self.row().name, title = %_req.title), err)]
    async fn lyric_fetch(&self, _req: &LyricSearchRequest) -> Result<Option<LyricDto>> {
        Ok(None)
    }

    #[tracing::instrument(skip(self), fields(addon = %self.row().name, title = %_req.title), err)]
    async fn lyric_search(
        &self,
        _req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        Ok(vec![])
    }

    #[tracing::instrument(skip(self), fields(addon = %self.row().name, id = %_id), err)]
    async fn lyric_get_by_id(&self, _id: &str) -> Result<Option<LyricDto>> {
        Ok(None)
    }

    /// Stable lyric provider id, used as the prefix for composite lyric IDs
    /// (`{prefix}_{inner}`). Default uses the addon row's `name` lowercased.
    fn lyric_provider_id(&self) -> String {
        format!("addon_{}", self.row().id)
    }
}

/// Lookup all registered `AddonKind`s. Built once on registry construction.
pub fn registered_kinds() -> Vec<Box<dyn AddonKind>> {
    inventory::iter::<AddonKindRegistration>
        .into_iter()
        .map(|r| (r.0)())
        .collect()
}

/// Read-write registry of addon instances. Loaded from the `addons` table
/// on startup; `reload()` re-reads after CRUD operations.
#[derive(Clone)]
pub struct AddonRegistry {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    instances: Vec<Arc<dyn Addon>>,
}

impl AddonRegistry {
    /// Build the registry by reading all rows and instantiating each via its kind.
    pub async fn from_db(db: &sqlx::SqlitePool) -> Result<Self> {
        let kinds = registered_kinds();
        let rows = AddonRow::list(db).await?;
        let mut instances: Vec<Arc<dyn Addon>> = Vec::with_capacity(rows.len());
        for row in rows {
            let Some(kind) = kinds.iter().find(|k| k.id() == row.kind) else {
                tracing::warn!(
                    addon_id = %row.id,
                    kind = %row.kind,
                    "skipping addon row with unknown kind"
                );
                continue;
            };
            match kind.instantiate(&row) {
                Ok(addon) => instances.push(Arc::from(addon)),
                Err(e) => tracing::warn!(
                    addon_id = %row.id,
                    kind = %row.kind,
                    error = %e,
                    "failed to instantiate addon"
                ),
            }
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(Inner { instances })),
        })
    }

    /// Re-read the table and rebuild instances. Call after addon CRUD.
    pub async fn reload(&self, db: &sqlx::SqlitePool) -> Result<()> {
        let new = Self::from_db(db).await?;
        let new_inner = new.inner.read().await;
        let mut guard = self.inner.write().await;
        guard.instances = new_inner.instances.clone();
        Ok(())
    }

    /// All instantiated addons, in priority order (lower priority first).
    pub async fn list(&self) -> Vec<Arc<dyn Addon>> {
        let guard = self.inner.read().await;
        let mut out = guard.instances.clone();
        out.sort_by_key(|a| a.row().priority);
        out
    }

    /// Addons that have a given resource enabled (via `supported_resources()`).
    #[tracing::instrument(skip(self))]
    pub async fn for_resource(&self, resource: AddonResource) -> Vec<Arc<dyn Addon>> {
        let mut result = Vec::new();
        for addon in self.list().await {
            if addon.supported_resources().await.contains(&resource) {
                result.push(addon);
            }
        }
        result
    }

    /// Find an addon instance by ID.
    pub async fn get(&self, id: uuid::Uuid) -> Option<Arc<dyn Addon>> {
        let guard = self.inner.read().await;
        guard.instances.iter().find(|a| a.row().id == id).cloned()
    }

    /// Build a `RemoteMediaStream` for a `media_id` of the form
    /// `addon:{instance_uuid}:{local_id}`. Returns `None` if the prefix
    /// doesn't match or the addon isn't registered.
    pub async fn make_catalog_stream(
        &self,
        media_id: &str,
    ) -> Option<Box<dyn RemoteMediaStream>> {
        let rest = media_id.strip_prefix("addon:")?;
        let (uuid_str, local_id) = rest.split_once(':')?;
        let id = uuid::Uuid::parse_str(uuid_str).ok()?;
        let addon = self.get(id).await?;
        Some(Box::new(AddonCatalogStream {
            addon,
            local_id: local_id.to_string(),
        }))
    }

    // --- Meta dispatch ---

    /// Refresh metadata on `media` by walking every meta-enabled addon in
    /// priority order. The first applicable addon respects `force_refresh`
    /// (replacing existing values); subsequent applicable addons only fill
    /// `None`/empty fields. Mirrors `MetaProviderService::refresh_meta`.
    #[tracing::instrument(skip(self, ctx), fields(media_id = %media.id, kind = ?media.kind))]
    pub async fn refresh_meta(
        &self,
        media: &mut db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Result<()> {
        let addons = self.for_resource(AddonResource::Meta).await;
        let mut applicable: Vec<_> = Vec::new();
        for a in addons {
            if a.meta_supports(media).await {
                applicable.push(a);
            }
        }

        if applicable.is_empty() {
            return Ok(());
        }

        let mut first = true;
        for addon in &applicable {
            let replace = first && force_refresh;
            first = false;

            match addon.meta(media, ctx).await {
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
                            tracing::warn!(id = %media.id, error = %e, "failed to persist relations");
                        }
                    }
                }
                Ok(None) => continue,
                Err(e) => {
                    tracing::error!(addon = %addon.row().name, error = %e, "meta addon error");
                    continue;
                }
            }
        }
        media.refreshed_at = Some(chrono::Utc::now().naive_utc());
        Ok(())
    }

    /// Sync the hierarchy under a root media item. Returns all discovered
    /// child media that should be upserted. Mirrors
    /// `MetaProviderService::sync_hierarchy`.
    #[tracing::instrument(skip(self, ctx), fields(root_id = %root.id, kind = ?root.kind))]
    pub async fn sync_hierarchy(
        &self,
        root: &mut db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let addons = self.for_resource(AddonResource::Meta).await;
        let applicable: Vec<_> = addons
            .iter()
            .filter(|a| a.hierarchy_supports(root))
            .cloned()
            .collect();

        for addon in &applicable {
            let mut children = match addon.sync_children(root, ctx).await {
                Ok(Some(children)) => children,
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(id = %root.id, error = %e, "failed to sync hierarchy");
                    continue;
                }
            };

            if children.is_empty() {
                continue;
            }

            self.refresh_tree_meta(root, &mut children, ctx).await;

            if let Err(e) = addon.persist_children_metadata(root, &children, ctx).await
            {
                tracing::warn!(id = %root.id, error = %e, "failed to persist hierarchy metadata");
            }

            return Ok(children);
        }

        Ok(vec![])
    }

    async fn refresh_tree_meta(
        &self,
        series: &db::Media,
        children: &mut [db::Media],
        ctx: &AppContext,
    ) {
        for addon in self.for_resource(AddonResource::Meta).await {
            if !addon.meta_supports(series).await {
                continue;
            }
            if let Err(e) = addon.refresh_tree_meta(series, children, ctx).await {
                tracing::warn!(id = %series.id, error = %e, "failed to refresh tree metadata");
            }
        }
    }

    /// Run meta refresh + hierarchy sync over a batch of media. Mirrors
    /// `MetaProviderService::process`.
    pub async fn process_meta_batch(
        &self,
        media: Vec<db::Media>,
        ctx: &AppContext,
        force_refresh: bool,
        save: bool,
    ) -> Result<Vec<db::Media>> {
        use futures::stream::{self, StreamExt};
        let results: Vec<Vec<db::Media>> = stream::iter(media)
            .map(|m| self.process_meta_item(m, ctx, force_refresh))
            .buffer_unordered(10)
            .collect()
            .await;

        let batch: Vec<db::Media> = results.into_iter().flatten().collect();

        if save && !batch.is_empty() {
            db::Media::upsert(&ctx.db, &batch).await?;
        }

        Ok(batch)
    }

    async fn process_meta_item(
        &self,
        mut media: db::Media,
        ctx: &AppContext,
        force_refresh: bool,
    ) -> Vec<db::Media> {
        let mut batch = vec![];
        if let Err(e) = self.refresh_meta(&mut media, ctx, force_refresh).await {
            tracing::warn!(id = %media.id, error = %e, "failed to refresh metadata, keeping as-is");
            batch.push(media);
            return batch;
        }

        let any_hierarchy_supports = self
            .for_resource(AddonResource::Meta)
            .await
            .iter()
            .any(|a| a.hierarchy_supports(&media));

        if any_hierarchy_supports {
            batch.push(media.clone());
            match self.sync_hierarchy(&mut media, ctx).await {
                Ok(children) => batch.extend(children),
                Err(e) => {
                    tracing::warn!(id = %media.id, error = %e, "failed to sync hierarchy");
                }
            }
        } else {
            batch.push(media);
        }

        batch
    }

    // --- Search dispatch ---

    /// Search via the first addon that supports `kind`. Mirrors
    /// `SearchServiceManager::search`.
    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        for addon in self.for_resource(AddonResource::Search).await {
            if !addon.search_supports(kind).await {
                continue;
            }
            match addon.search(kind, query, limit, ctx).await {
                Ok(Some(results)) => return Ok(results),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "search addon error");
                }
            }
        }
        Ok(vec![])
    }

    /// Try every search addon's `persist_search_result`; return first owner.
    /// Mirrors `SearchServiceManager::persist`.
    pub async fn persist_search_result(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        for addon in self.for_resource(AddonResource::Search).await {
            if let Some(media) = addon.persist_search_result(id, ctx).await? {
                return Ok(Some(media));
            }
        }
        Ok(None)
    }

    // --- Subtitles dispatch ---

    /// Aggregate subtitles from every subtitle-enabled addon that supports `media`.
    pub async fn fetch_subtitles(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Vec<sdks::stremio::Subtitle> {
        let mut subs = vec![];
        for addon in self.for_resource(AddonResource::Subtitles).await {
            if !addon.subtitles_supports(media) {
                continue;
            }
            match addon.subtitles(media, db).await {
                Ok(s) => subs.extend(s),
                Err(e) => tracing::warn!(
                    addon = %addon.row().name,
                    error = %e,
                    "subtitle addon failed"
                ),
            }
        }
        subs
    }

    // --- Streams dispatch ---

    /// Resolve playable streams from the first stream-enabled addon that
    /// Collect streams from ALL stream-enabled addons in priority order, merging
    /// results. Every addon that supports the media kind is queried; results are
    /// concatenated so the client can choose from streams across multiple sources.
    #[tracing::instrument(skip(self, ctx, media), fields(media_id = %media.id, title = %media.title, kind = ?media.kind))]
    pub async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let addons = self.for_resource(AddonResource::Streams).await;
        if addons.is_empty() {
            tracing::debug!("no stream addons registered");
            return Ok(vec![]);
        }
        let mut all_streams: Vec<db::Media> = Vec::new();
        for addon in addons {
            if !addon.streams_supports(media) {
                tracing::debug!(addon = %addon.row().name, "addon does not support this media kind, skipping");
                continue;
            }
            tracing::debug!(addon = %addon.row().name, "querying stream addon");
            match addon.streams(media, ctx).await {
                Ok(streams) => {
                    tracing::debug!(addon = %addon.row().name, count = streams.len(), "streams from addon");
                    all_streams.extend(streams);
                }
                Err(e) => tracing::warn!(
                    addon = %addon.row().name,
                    error = %e,
                    "stream addon failed"
                ),
            }
        }
        tracing::debug!(total = all_streams.len(), "streams collected");
        Ok(all_streams)
    }

    /// Resolve streams for `media`, persist them as `Source` children, and
    /// stamp `streams_refreshed_at`. Skipped if the stamp is fresh.
    /// Mirrors `StreamServiceManager::refresh_sources`.
    pub async fn refresh_sources(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<()> {
        const STREAMS_TTL_SECS: i64 = 3600;

        if let Some(refreshed) = media.streams_refreshed_at {
            let age = chrono::Utc::now().naive_utc() - refreshed;
            if age.num_seconds() < STREAMS_TTL_SECS {
                tracing::debug!(id = %media.id, age_secs = age.num_seconds(), "streams fresh, skipping refresh");
                return Ok(());
            }
        }

        let raw = self.get_streams(media, ctx).await?;
        if raw.is_empty() {
            return Ok(());
        }

        let now = chrono::Utc::now().naive_utc();
        let sources: Vec<db::Media> = raw
            .into_iter()
            .enumerate()
            .map(|(idx, mut s)| {
                s.id =
                    uuid::Uuid::new_v5(&media.id, format!("source_{idx}").as_bytes());
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

    // --- Segments dispatch ---

    /// Fetch and merge timeline segments from all segment-enabled addons.
    pub async fn get_segments(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> MediaSegments {
        let mut merged = MediaSegments::default();
        for addon in self.for_resource(AddonResource::Segment).await {
            if !addon.segments_supports(media) {
                continue;
            }
            match addon.segments(media, ctx).await {
                Ok(segs) if !segs.is_empty() => {
                    tracing::debug!(addon = %addon.row().name, item = %media.id, seg_count = segs.to_pairs().len(), "segments fetched");
                    merged.merge_from(segs);
                }
                Ok(_) => {}
                Err(e) => tracing::error!(
                    addon = %addon.row().name,
                    item = %media.id,
                    error = %e,
                    "segment addon failed"
                ),
            }
        }
        merged
    }

    // --- Lyrics dispatch ---

    pub async fn lyric_fetch(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Option<LyricDto>> {
        for addon in self.for_resource(AddonResource::Lyrics).await {
            match addon.lyric_fetch(req).await {
                Ok(Some(l)) => return Ok(Some(l)),
                Ok(None) => continue,
                Err(e) => tracing::warn!(
                    addon = %addon.row().name,
                    error = %e,
                    "lyric addon fetch failed"
                ),
            }
        }
        Ok(None)
    }

    pub async fn lyric_search(
        &self,
        req: &LyricSearchRequest,
    ) -> Result<Vec<RemoteLyricInfoDto>> {
        let mut out = Vec::new();
        for addon in self.for_resource(AddonResource::Lyrics).await {
            match addon.lyric_search(req).await {
                Ok(items) => out.extend(items),
                Err(e) => tracing::warn!(
                    addon = %addon.row().name,
                    error = %e,
                    "lyric addon search failed"
                ),
            }
        }
        Ok(out)
    }

    /// `composite_id` format: `{provider_prefix}_{inner_id}`.
    pub async fn lyric_get_by_composite_id(
        &self,
        composite_id: &str,
    ) -> Result<Option<LyricDto>> {
        for addon in self.for_resource(AddonResource::Lyrics).await {
            let prefix = format!("{}_", addon.lyric_provider_id());
            if let Some(inner) = composite_id.strip_prefix(&prefix) {
                return addon.lyric_get_by_id(inner).await;
            }
        }
        Ok(None)
    }
}

/// Adapter so an `Addon`'s catalog can plug into the existing
/// `RemoteMediaStream`-based import pipeline.
struct AddonCatalogStream {
    addon: Arc<dyn Addon>,
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
                anyhow!(
                    "addon {} ({}) does not serve catalog '{}'",
                    self.addon.row().name,
                    self.addon.row().kind,
                    self.local_id
                )
            })
    }
}

/// Format the `media.media_id` value for a catalog row coming from an addon.
pub fn make_media_id(addon_id: uuid::Uuid, local_id: &str) -> String {
    format!("addon:{addon_id}:{local_id}")
}
