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

#[derive(Debug, Clone)]
pub struct CatalogInfo {
    pub provider_catalog_id: String,
    pub name: String,
}

#[async_trait]
pub trait RemoteMediaStream: Send + Sync {
    async fn stream(
        &self,
        ctx: &AppContext,
    ) -> Result<Pin<Box<dyn Stream<Item = db::Media> + Send>>>;
}

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

pub struct AddonKindRegistration(pub fn() -> Box<dyn AddonKind>);
inventory::collect!(AddonKindRegistration);

pub trait AddonKind: Send + Sync {
    fn id(&self) -> &'static str;
    fn metadata(&self) -> AddonKindMetadata;
    fn instantiate(&self, row: &AddonRow) -> Result<AddonInstance>;
}

#[async_trait]
pub trait Addon: Send + Sync {
    fn row(&self) -> &AddonRow;
    async fn supported_resources(&self) -> Vec<AddonResource> {
        self.row().resources.clone()
    }
    async fn supported_types(&self) -> Vec<String> {
        registered_kinds()
            .into_iter()
            .find(|k| k.id() == self.row().kind)
            .map(|k| k.metadata().supported_types)
            .unwrap_or_default()
    }
}

#[async_trait]
pub trait CatalogAddon: Addon {
    async fn stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>>;
    async fn list(&self, ctx: &AppContext) -> Result<Vec<CatalogInfo>>;
}

#[async_trait]
pub trait MetaAddon: Addon {
    async fn supports(&self, media: &db::Media) -> bool;
    async fn fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<MetaResult>>;
    async fn refresh_tree(
        &self,
        _root: &db::Media,
        _children: &mut [db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait HierarchyAddon: Addon {
    fn supports(&self, root: &db::Media) -> bool;
    async fn sync_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
    async fn persist_children_metadata(
        &self,
        _root: &db::Media,
        _children: &[db::Media],
        _ctx: &AppContext,
    ) -> Result<()> {
        Ok(())
    }
}

#[async_trait]
pub trait SearchAddon: Addon {
    async fn supports(&self, kind: &db::MediaKind) -> bool;
    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>>;
    async fn persist_result(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>>;
}

#[async_trait]
pub trait SubtitleAddon: Addon {
    fn supports(&self, media: &db::Media) -> bool;
    async fn fetch(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Result<Vec<sdks::stremio::Subtitle>>;
}

#[async_trait]
pub trait StreamAddon: Addon {
    fn supports(&self, media: &db::Media) -> bool;
    async fn resolve(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>>;
}

#[async_trait]
pub trait SegmentAddon: Addon {
    fn supports(&self, media: &db::Media) -> bool;
    async fn fetch(&self, media: &db::Media, ctx: &AppContext)
    -> Result<MediaSegments>;
}

#[async_trait]
pub trait LyricAddon: Addon {
    async fn fetch(&self, req: &LyricSearchRequest) -> Result<Option<LyricDto>>;
    async fn search(&self, req: &LyricSearchRequest)
    -> Result<Vec<RemoteLyricInfoDto>>;
    async fn get_by_id(&self, id: &str) -> Result<Option<LyricDto>>;
    fn provider_id(&self) -> String {
        format!("addon_{}", self.row().id)
    }
}

pub struct AddonInstance {
    pub addon: Arc<dyn Addon>,
    pub catalog: Option<Arc<dyn CatalogAddon>>,
    pub meta: Option<Arc<dyn MetaAddon>>,
    pub hierarchy: Option<Arc<dyn HierarchyAddon>>,
    pub search: Option<Arc<dyn SearchAddon>>,
    pub subtitle: Option<Arc<dyn SubtitleAddon>>,
    pub stream: Option<Arc<dyn StreamAddon>>,
    pub segment: Option<Arc<dyn SegmentAddon>>,
    pub lyric: Option<Arc<dyn LyricAddon>>,
}

pub fn registered_kinds() -> Vec<Box<dyn AddonKind>> {
    inventory::iter::<AddonKindRegistration>
        .into_iter()
        .map(|r| (r.0)())
        .collect()
}

#[derive(Clone)]
pub struct AddonRegistry {
    inner: Arc<RwLock<Inner>>,
}

pub struct Inner {
    pub addons: Vec<Arc<dyn Addon>>,
    pub catalogs: Vec<Arc<dyn CatalogAddon>>,
    pub meta: Vec<Arc<dyn MetaAddon>>,
    pub hierarchy: Vec<Arc<dyn HierarchyAddon>>,
    pub search: Vec<Arc<dyn SearchAddon>>,
    pub subtitles: Vec<Arc<dyn SubtitleAddon>>,
    pub streams: Vec<Arc<dyn StreamAddon>>,
    pub segments: Vec<Arc<dyn SegmentAddon>>,
    pub lyrics: Vec<Arc<dyn LyricAddon>>,
}

impl AddonRegistry {
    pub async fn from_db(db: &sqlx::SqlitePool) -> Result<Self> {
        let kinds = registered_kinds();
        let rows = AddonRow::list(db).await?;
        let mut inner = Inner {
            addons: vec![],
            catalogs: vec![],
            meta: vec![],
            hierarchy: vec![],
            search: vec![],
            subtitles: vec![],
            streams: vec![],
            segments: vec![],
            lyrics: vec![],
        };
        for row in rows {
            let Some(kind) = kinds.iter().find(|k| k.id() == row.kind) else {
                tracing::warn!(addon_id = %row.id, kind = %row.kind, "skipping addon row with unknown kind");
                continue;
            };
            match kind.instantiate(&row) {
                Ok(instance) => {
                    inner.addons.push(instance.addon);
                    if let Some(v) = instance.catalog {
                        inner.catalogs.push(v);
                    }
                    if let Some(v) = instance.meta {
                        inner.meta.push(v);
                    }
                    if let Some(v) = instance.hierarchy {
                        inner.hierarchy.push(v);
                    }
                    if let Some(v) = instance.search {
                        inner.search.push(v);
                    }
                    if let Some(v) = instance.subtitle {
                        inner.subtitles.push(v);
                    }
                    if let Some(v) = instance.stream {
                        inner.streams.push(v);
                    }
                    if let Some(v) = instance.segment {
                        inner.segments.push(v);
                    }
                    if let Some(v) = instance.lyric {
                        inner.lyrics.push(v);
                    }
                }
                Err(e) => {
                    tracing::warn!(addon_id = %row.id, kind = %row.kind, error = %e, "failed to instantiate addon")
                }
            }
        }
        Ok(Self {
            inner: Arc::new(RwLock::new(inner)),
        })
    }

    pub async fn reload(&self, db: &sqlx::SqlitePool) -> Result<()> {
        let new = Self::from_db(db).await?;
        let new_inner = new.inner.read().await;
        let mut guard = self.inner.write().await;
        *guard = Inner {
            addons: new_inner.addons.clone(),
            catalogs: new_inner.catalogs.clone(),
            meta: new_inner.meta.clone(),
            hierarchy: new_inner.hierarchy.clone(),
            search: new_inner.search.clone(),
            subtitles: new_inner.subtitles.clone(),
            streams: new_inner.streams.clone(),
            segments: new_inner.segments.clone(),
            lyrics: new_inner.lyrics.clone(),
        };
        Ok(())
    }

    pub async fn list(&self) -> Vec<Arc<dyn Addon>> {
        let guard = self.inner.read().await;
        let mut out = guard.addons.clone();
        out.sort_by_key(|a| a.row().priority);
        out
    }

    pub async fn for_resource(&self, resource: AddonResource) -> Vec<Arc<dyn Addon>> {
        let mut result = Vec::new();
        for addon in self.list().await {
            if addon.supported_resources().await.contains(&resource) {
                result.push(addon);
            }
        }
        result
    }

    pub async fn get(&self, id: uuid::Uuid) -> Option<Arc<dyn Addon>> {
        let guard = self.inner.read().await;
        guard.addons.iter().find(|a| a.row().id == id).cloned()
    }

    pub async fn catalog_addons(&self) -> Vec<Arc<dyn CatalogAddon>> {
        let guard = self.inner.read().await;
        guard.catalogs.clone()
    }

    pub async fn get_catalog(&self, id: uuid::Uuid) -> Option<Arc<dyn CatalogAddon>> {
        let guard = self.inner.read().await;
        guard.catalogs.iter().find(|a| a.row().id == id).cloned()
    }

    pub async fn make_catalog_stream(
        &self,
        media_id: &str,
    ) -> Option<Box<dyn RemoteMediaStream>> {
        let rest = media_id.strip_prefix("addon:")?;
        let (uuid_str, local_id) = rest.split_once(':')?;
        let id = uuid::Uuid::parse_str(uuid_str).ok()?;
        let guard = self.inner.read().await;
        let addon = guard.catalogs.iter().find(|a| a.row().id == id).cloned()?;
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
    ) -> Result<()> {
        let guard = self.inner.read().await;
        let mut applicable = Vec::new();
        for a in &guard.meta {
            if a.supports(media).await {
                applicable.push(a.clone());
            }
        }
        drop(guard);
        if applicable.is_empty() {
            return Ok(());
        }
        let mut first = true;
        for addon in &applicable {
            let replace = first && force_refresh;
            first = false;
            match addon.fetch(media, ctx).await {
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

    pub async fn sync_hierarchy(
        &self,
        root: &mut db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let guard = self.inner.read().await;
        let applicable: Vec<_> = guard
            .hierarchy
            .iter()
            .filter(|a| a.supports(root))
            .cloned()
            .collect();
        drop(guard);
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
        let guard = self.inner.read().await;
        let meta = guard.meta.clone();
        drop(guard);
        for addon in meta {
            if !addon.supports(series).await {
                continue;
            }
            if let Err(e) = addon.refresh_tree(series, children, ctx).await {
                tracing::warn!(id = %series.id, error = %e, "failed to refresh tree metadata");
            }
        }
    }

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
        let guard = self.inner.read().await;
        let any_hierarchy_supports = guard.hierarchy.iter().any(|a| a.supports(&media));
        drop(guard);
        if any_hierarchy_supports {
            batch.push(media.clone());
            match self.sync_hierarchy(&mut media, ctx).await {
                Ok(children) => batch.extend(children),
                Err(e) => {
                    tracing::warn!(id = %media.id, error = %e, "failed to sync hierarchy")
                }
            }
        } else {
            batch.push(media);
        }
        batch
    }

    pub async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
        let guard = self.inner.read().await;
        let addons = guard.search.clone();
        drop(guard);
        for addon in addons {
            if !addon.supports(kind).await {
                continue;
            }
            match addon.search(kind, query, limit, ctx).await {
                Ok(Some(results)) => return Ok(results),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "search addon error")
                }
            }
        }
        Ok(vec![])
    }

    pub async fn persist_search_result(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let guard = self.inner.read().await;
        let addons = guard.search.clone();
        drop(guard);
        for addon in addons {
            if let Some(media) = addon.persist_result(id, ctx).await? {
                return Ok(Some(media));
            }
        }
        Ok(None)
    }

    pub async fn fetch_subtitles(
        &self,
        media: &db::Media,
        db: &SqlitePool,
    ) -> Vec<sdks::stremio::Subtitle> {
        let guard = self.inner.read().await;
        let addons = guard.subtitles.clone();
        drop(guard);
        let mut subs = vec![];
        for addon in addons {
            if !addon.supports(media) {
                continue;
            }
            match addon.fetch(media, db).await {
                Ok(s) => subs.extend(s),
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "subtitle addon failed")
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
        let addons = guard.streams.clone();
        drop(guard);
        let mut all_streams: Vec<db::Media> = Vec::new();
        for addon in addons {
            if !addon.supports(media) {
                continue;
            }
            match addon.resolve(media, ctx).await {
                Ok(streams) => all_streams.extend(streams),
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "stream addon failed")
                }
            }
        }
        Ok(all_streams)
    }

    pub async fn refresh_sources(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<()> {
        const STREAMS_TTL_SECS: i64 = 3600;
        if let Some(refreshed) = media.streams_refreshed_at {
            let age = chrono::Utc::now().naive_utc() - refreshed;
            if age.num_seconds() < STREAMS_TTL_SECS {
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
        sqlx::query("DELETE FROM media WHERE kind = 'stream' AND parent_id = ? AND updated_at < datetime('now', '-7 days')").bind(media.id).execute(&ctx.db).await?;
        Ok(())
    }

    pub async fn get_segments(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> MediaSegments {
        let guard = self.inner.read().await;
        let addons = guard.segments.clone();
        drop(guard);
        let mut merged = MediaSegments::default();
        for addon in addons {
            if !addon.supports(media) {
                continue;
            }
            match addon.fetch(media, ctx).await {
                Ok(segs) if !segs.is_empty() => merged.merge_from(segs),
                Ok(_) => {}
                Err(e) => {
                    tracing::error!(addon = %addon.row().name, item = %media.id, error = %e, "segment addon failed")
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
        let addons = guard.lyrics.clone();
        drop(guard);
        for addon in addons {
            match addon.fetch(req).await {
                Ok(Some(l)) => return Ok(Some(l)),
                Ok(None) => continue,
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "lyric addon fetch failed")
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
        let addons = guard.lyrics.clone();
        drop(guard);
        let mut out = Vec::new();
        for addon in addons {
            match addon.search(req).await {
                Ok(items) => out.extend(items),
                Err(e) => {
                    tracing::warn!(addon = %addon.row().name, error = %e, "lyric addon search failed")
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
        let addons = guard.lyrics.clone();
        drop(guard);
        for addon in addons {
            let prefix = format!("{}_", addon.provider_id());
            if let Some(inner) = composite_id.strip_prefix(&prefix) {
                return addon.get_by_id(inner).await;
            }
        }
        Ok(None)
    }
}

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
            .stream(ctx, &self.local_id)
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

pub fn make_media_id(addon_id: uuid::Uuid, local_id: &str) -> String {
    format!("addon:{addon_id}:{local_id}")
}
