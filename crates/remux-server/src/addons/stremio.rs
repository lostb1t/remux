use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt};
use nutype::nutype;

use serde::{Deserialize, Deserializer};
use sqlx::SqlitePool;
use std::{
    pin::Pin,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, CatalogAddon, CatalogInfo, MediaKind,
    MetaAddon, ResourceType, SearchAddon, StreamAddon, SubtitleAddon, SubtitleInfo,
    TreeAddon, addon,
};
use crate::{
    AppContext, common, db, sdks,
    sdks::{CachedEndpoint, ClientError},
    services::{MediaResolveService, stremio as stremio_service},
};

pub struct StremioPreset;

impl AddonPreset for StremioPreset {
    fn id(&self) -> &'static str {
        "stremio"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "stremio".to_string(),
            display_name: "Stremio addon".to_string(),
            description: "Any addon that speaks the Stremio addon protocol \
                          (manifest.json + /catalog endpoints). Includes AIO."
                .to_string(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Catalog),
                AddonMetadata::simple_resource(ResourceType::Meta),
                AddonMetadata::simple_resource(ResourceType::Search),
                AddonMetadata::simple_resource(ResourceType::Subtitles),
                AddonMetadata::simple_resource(ResourceType::Stream),
            ],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
            supported_resources_user: vec![
                ResourceType::Search,
                ResourceType::Subtitles,
                ResourceType::Stream,
            ],
            supported_types_user: vec![MediaKind::Movie, MediaKind::Series],
            options: vec![
                AddonOption {
                    id: "manifest_url".to_string(),
                    name: "Manifest URL".to_string(),
                    description: Some("Full URL to the addon's manifest.json".to_string()),
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "strict_stream_matching".to_string(),
                    name: "Strict stream matching".to_string(),
                    description: Some(
                        "Filter streams by title, year, and episode filename. Enable only for addons that return unrelated or mismatched results."
                            .to_string(),
                    ),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
                AddonOption {
                    id: "resolve_debrid_packs_as_torrents".to_string(),
                    name: "Resolve debrid packs as torrents".to_string(),
                    description: Some(
                        "Convert unresolved debrid season packs into native torrents so remux can select the episode. This bypasses debrid playback and uses direct P2P."
                            .to_string(),
                    ),
                    required: false,
                    default: Some(serde_json::Value::Bool(false)),
                    kind: AddonOptionType::Boolean,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        _addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let raw_url = cfg
            .get("manifest_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Stremio addon missing manifest_url in config"))?
            .to_string();
        let manifest_url = StremioManifestUrl::try_new(raw_url)
            .map_err(|e| anyhow!("Invalid manifest_url: {e}"))?;
        let strict_stream_matching = cfg
            .get("strict_stream_matching")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let resolve_debrid_packs_as_torrents = cfg
            .get("resolve_debrid_packs_as_torrents")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let client = super::make_http_client();
        let addon = Arc::new(StremioAddon {
            manifest_url,
            strict_stream_matching,
            resolve_debrid_packs_as_torrents,
            client,
            medias_cache: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            meta: Some(addon.clone()),
            search: Some(addon.clone()),
            subtitle: Some(addon.clone()),
            stream: Some(addon.clone()),
            tree: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(StremioPreset))
}

pub(super) fn parse_manifest_info(
    manifest: &remux_sdks::stremio::Manifest,
) -> (
    Vec<remux_sdks::stremio::ResourceRef>,
    Vec<remux_sdks::stremio::MediaType>,
) {
    let mut seen_names: Vec<ResourceType> = Vec::new();
    let mut resources: Vec<remux_sdks::stremio::ResourceRef> = Vec::new();

    for res in manifest
        .resources
        .iter()
        .cloned()
    {
        let name = res.resource_type();
        if seen_names.contains(&name) {
            continue;
        }
        seen_names.push(name.clone());
        resources.push(res.into_ref());
    }

    // Detect search support via catalog extras and synthesise a Search resource if needed.
    if manifest
        .catalogs
        .iter()
        .any(|c| {
            c.extra
                .iter()
                .any(|e| e.name == "search")
        })
        && !seen_names.contains(&ResourceType::Search)
    {
        resources.push(remux_sdks::stremio::ResourceRef {
            name: ResourceType::Search,
            types: vec![],
            id_prefixes: None,
        });
    }

    let types = manifest
        .types
        .iter()
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s.clone()))
                .unwrap_or(remux_sdks::stremio::MediaType::Other(s.clone()))
        })
        .collect();
    (resources, types)
}

#[nutype(
    sanitize(trim, with = |s: String| {
        // Parse as a URL so we strip /manifest.json and /configure from the path
        // even when a query string is present (a plain suffix match would miss
        // "…/manifest.json?apikey=abc" because the string doesn't end with the suffix).
        if let Ok(mut url) = url::Url::parse(s.trim()) {
            let path = url
                .path()
                .trim_end_matches('/')
                .trim_end_matches("/manifest.json")
                .trim_end_matches("/configure")
                .to_string();
            url.set_path(&path);
            return url.to_string();
        }
        // Fallback for bare paths / non-URL strings.
        let s = s.trim_end_matches('/');
        let s = s.strip_suffix("/manifest.json").unwrap_or(s);
        s.strip_suffix("/configure").unwrap_or(s).to_string()
    }),
    validate(not_empty),
    derive(Debug, Clone, PartialEq, Display, Serialize, Deserialize, AsRef, Deref)
)]
pub struct StremioManifestUrl(String);

fn deserialize_option_aio_url<'de, D>(
    de: D,
) -> Result<Option<StremioManifestUrl>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(de)?;
    Ok(raw.and_then(|s| StremioManifestUrl::try_new(s).ok()))
}

pub struct StremioAddon {
    manifest_url: StremioManifestUrl,
    strict_stream_matching: bool,
    resolve_debrid_packs_as_torrents: bool,
    client: reqwest::Client,
    /// Raw Stremio `Meta` cached per series lookup-id for the duration of one tree sync.
    /// Shared between the tree-children path and `stremio_meta_fetch` so the API is
    /// called exactly once per series. Evicted by `on_series_done`.
    medias_cache: Arc<
        std::sync::Mutex<std::collections::HashMap<String, Arc<sdks::stremio::Meta>>>,
    >,
}

impl StremioAddon {
    fn service(&self) -> Result<stremio_service::StremioService> {
        stremio_service::StremioService::from_url(&self.manifest_url)
    }
}

#[async_trait]
impl AddonKind for StremioAddon {
    fn id(&self) -> &'static str {
        "stremio"
    }

    async fn available_info(
        &self,
    ) -> Result<
        Option<(
            Vec<remux_sdks::stremio::ResourceRef>,
            Vec<remux_sdks::stremio::MediaType>,
        )>,
    > {
        let svc = self.service()?;
        let manifest = svc
            .get_manifest()
            .await?;
        Ok(Some(parse_manifest_info(&manifest)))
    }
}

#[async_trait]
impl CatalogAddon for StremioAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let svc = self.service()?;
        let manifest = svc
            .get_manifest()
            .await?;
        Ok(manifest
            .catalogs
            .into_iter()
            .filter(|c| {
                !c.id
                    .contains("search")
            })
            .map(|c| {
                let kind_label = {
                    let k = c
                        .kind
                        .trim();
                    let mut chars = k.chars();
                    match chars.next() {
                        Some(first) => {
                            first
                                .to_uppercase()
                                .collect::<String>()
                                + chars.as_str()
                        }
                        None => String::new(),
                    }
                };
                let stremio_kind: remux_sdks::stremio::MediaType =
                    serde_json::from_value(serde_json::Value::String(
                        c.kind
                            .clone(),
                    ))
                    .unwrap_or(
                        remux_sdks::stremio::MediaType::Other(
                            c.kind
                                .clone(),
                        ),
                    );
                CatalogInfo {
                    collection_media_kind: matches!(
                        c.kind
                            .trim()
                            .to_lowercase()
                            .as_str(),
                        "movie" | "series" | "episode" | "album" | "artist" | "track"
                    )
                    .then(|| {
                        c.kind
                            .as_str()
                            .into()
                    }),
                    media_kind: db::MediaKind::try_from(stremio_kind).ok(),
                    ..CatalogInfo::new(
                        format!("{}:{}", c.kind, c.id),
                        format!(
                            "{} — {} — {}",
                            manifest
                                .name
                                .trim(),
                            c.name
                                .trim(),
                            kind_label
                        ),
                    )
                }
            })
            .collect())
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let svc = self.service()?;

        let (kind, id) = local_id
            .split_once(':')
            .ok_or_else(|| anyhow!("invalid stremio catalog id: '{}'", local_id))?;

        let manifest = svc
            .get_manifest()
            .await?;
        let supports_skip = manifest
            .get_catalog(id, &kind.to_string())
            .map(|cat| {
                cat.extra
                    .iter()
                    .any(|e| e.name == "skip")
            })
            .unwrap_or(false);

        let stream = svc
            .get_catalog_stream(kind.to_string(), id.to_string(), supports_skip)
            .await?;
        let tmdb_client = crate::common::tmdb_client(
            &ctx.db,
            &ctx.config
                .tmdb_base_url,
        )
        .await;

        let stream = stream
            .map(move |mut meta| {
                let svc = svc.clone();
                let tmdb = tmdb_client.clone();
                async move {
                    if meta.is_error() {
                        debug!(id = %meta.id, "catalog item is an error stub, skipping");
                        return vec![];
                    }
                    if !resolve_imdb_id(&mut meta, Some(&svc), tmdb.as_ref()).await {
                        debug!(id = %meta.id, "could not resolve imdb_id, skipping");
                        return vec![];
                    }
                    match db::stremio_meta_to_medias(meta) {
                        Ok(mut items) => {
                            // Only emit the top-level item (series/movie).
                            // Seasons and episodes are populated by sync_tree
                            // during RefreshLibrary, avoiding FK constraint
                            // failures when chunks are split across parents.
                            items.retain(|x| x.parent_id.is_none());
                            if let Some(top) = items.first_mut() {
                                top.parent_id = None;
                            }
                            items
                        }
                        Err(e) => {
                            warn!(error = %e, "failed to convert stremio metadata, skipping");
                            vec![]
                        }
                    }
                }
            })
            .buffer_unordered(10)
            .flat_map(futures::stream::iter);

        Ok(Some(Box::pin(stream)))
    }
}

#[async_trait]
impl MetaAddon for StremioAddon {
    async fn supports(&self, media: &db::Media) -> bool {
        stremio_type_for_kind(&media.kind).is_some()
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        _config: &crate::api::ServerConfiguration,
    ) -> Result<Option<db::Media>> {
        let svc = self.service()?;
        stremio_meta_fetch(&svc, media, ctx, &self.medias_cache).await
    }

    fn on_series_done(&self, meta_id: &str) {
        self.medias_cache
            .lock()
            .unwrap()
            .remove(meta_id);
    }
}

#[async_trait]
impl TreeAddon for StremioAddon {
    fn supports(&self, root: &db::Media) -> bool {
        matches!(root.kind, db::MediaKind::Series | db::MediaKind::Season)
    }

    async fn get_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        match root.kind {
            db::MediaKind::Series => {
                let svc = self.service()?;
                let meta_arc =
                    fetch_and_cache_meta(&svc, root, &self.medias_cache, ctx).await?;
                let seasons =
                    db::stremio_meta_seasons(&meta_arc, root.id, &root.external_ids);
                if seasons.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(seasons))
                }
            }
            db::MediaKind::Season => {
                // The series meta is cached under the series' own lookup ID.
                // Prefer grandparent (always set during process_tree_root) over
                // the season's own external_ids which carry no series-level ID.
                let meta_id = root
                    .grandparent
                    .as_deref()
                    .and_then(|gp| {
                        gp.external_ids
                            .stremio_lookup_id()
                    })
                    .or_else(|| {
                        root.external_ids
                            .stremio_lookup_id()
                    })
                    .ok_or_else(|| {
                        anyhow!("season {} has no resolvable meta id", root.id)
                    })?;
                let meta_arc = self
                    .medias_cache
                    .lock()
                    .unwrap()
                    .get(&meta_id)
                    .cloned();
                let Some(meta_arc) = meta_arc else {
                    return Ok(None);
                };
                let season_idx = match root.idx {
                    Some(i) => i,
                    None => return Ok(None),
                };
                let series_external_ids = root
                    .grandparent
                    .as_deref()
                    .map(|gp| {
                        gp.external_ids
                            .clone()
                    })
                    .ok_or_else(|| anyhow!("season {} missing grandparent", root.id))?;
                let series_id = root
                    .grandparent_id
                    .ok_or_else(|| {
                        anyhow!("season {} missing grandparent_id", root.id)
                    })?;
                let mut episodes = db::stremio_meta_season_episodes(
                    &meta_arc,
                    series_id,
                    root.id,
                    season_idx,
                    &series_external_ids,
                )?;
                let now = chrono::Utc::now().naive_utc();
                for ep in &mut episodes {
                    // Mark refreshed so TMDB isn't called per-episode during tree sync.
                    ep.refreshed_at = Some(now);
                }
                if episodes.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(episodes))
                }
            }
            _ => Ok(None),
        }
    }
}

#[async_trait]
impl SearchAddon for StremioAddon {
    async fn search_supports(&self, kind: &db::MediaKind) -> bool {
        stremio_type_for_kind(kind).is_some()
    }

    async fn search(
        &self,
        kind: &db::MediaKind,
        query: &str,
        limit: usize,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        let svc = self.service()?;
        let results = stremio_search(&svc, kind, query, limit, ctx).await?;
        Ok(Some(results))
    }
}

#[async_trait]
impl SubtitleAddon for StremioAddon {
    fn supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
    }

    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        _db: &SqlitePool,
    ) -> Result<Vec<SubtitleInfo>> {
        let svc = self.service()?;
        let subs = stremio_subtitles(&svc, media).await?;
        Ok(subs
            .into_iter()
            .map(|s| SubtitleInfo {
                id: s.id,
                url: Some(crate::stream::StreamDescriptor::http(s.url)),
                lang: s.lang,
                is_forced: false,
                is_hi: false,
            })
            .collect())
    }
}

#[async_trait]
impl StreamAddon for StremioAddon {
    fn supports(&self, media: &db::Media) -> bool {
        stremio_type_for_kind(&media.kind).is_some()
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        _ctx: &AppContext,
        id_prefixes: Option<&[String]>,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let svc = self.service()?;
        stremio_streams(
            &svc,
            &self.manifest_url,
            media,
            id_prefixes,
            self.strict_stream_matching,
            self.resolve_debrid_packs_as_torrents,
        )
        .await
    }
}

fn stremio_type_for_kind(kind: &db::MediaKind) -> Option<&'static str> {
    match kind {
        db::MediaKind::Movie => Some("movie"),
        db::MediaKind::Series | db::MediaKind::Season | db::MediaKind::Episode => {
            Some("series")
        }
        db::MediaKind::Track => Some("track"),
        db::MediaKind::Album => Some("album"),
        db::MediaKind::Artist => Some("artist"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Catalog helpers
// ---------------------------------------------------------------------------

pub(crate) async fn resolve_imdb_id<A: sdks::Auth + Clone>(
    meta: &mut sdks::stremio::Meta,
    svc: Option<&stremio_service::StremioService>,
    tmdb_client: Option<&sdks::RestClient<A>>,
) -> bool {
    let t = Instant::now();

    // Phase 1: build the richest possible ExternalIds before any TMDB calls.
    let mut ids = db::ExternalIds::from_stremio_id(&meta.id);
    if ids
        .imdb
        .is_none()
    {
        ids.imdb = meta
            .imdb_id
            .as_deref()
            .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok());
    }
    if ids
        .tmdb
        .is_none()
    {
        ids.tmdb = meta
            .moviedb_id
            .map(|n| n as i64);
    }

    // AIO resolve: the addon may map its own ID to an IMDB ID.
    if ids
        .imdb
        .is_none()
    {
        if let Some(svc) = svc {
            match meta
                .resolve(&svc.client)
                .await
            {
                Ok(()) => {}
                Err(e) => warn!(id = %meta.id, error = %e, "AIO resolve failed"),
            }
            debug!(id = %meta.id, elapsed = ?t.elapsed(), resolved = meta.imdb_id.is_some(), "after AIO resolve");
            ids.imdb = meta
                .imdb_id
                .as_deref()
                .and_then(|s| db::NonEmptyString::try_new(s.to_string()).ok());
        }
    }

    // Phase 2: single TMDB resolution pass (TMDB/TVDB/Kitsu chains handled inside).
    if ids
        .imdb
        .is_none()
    {
        if let Some(client) = tmdb_client {
            if !ids.is_empty() {
                let is_tv = meta.media_type == sdks::stremio::MediaType::Series;
                ids.imdb =
                    MediaResolveService::resolve_imdb_from_ids(&ids, is_tv, client)
                        .await;
                debug!(id = %meta.id, elapsed = ?t.elapsed(), resolved = ids.imdb.is_some(), "after TMDB resolve");
            }
        }
    }

    meta.imdb_id = ids
        .imdb
        .clone()
        .map(Into::into);

    if meta
        .imdb_id
        .is_none()
    {
        // Allow items that have a recognised non-IMDB identity (custom addon prefix or
        // kitsu ID that couldn't be resolved to IMDB — anime often isn't on IMDB).
        return ids
            .custom_stremio_id
            .is_some()
            || ids
                .kitsu
                .is_some();
    }

    true
}

fn is_404(e: &anyhow::Error) -> bool {
    matches!(
        e.downcast_ref::<ClientError>(),
        Some(ClientError::Http { status: 404, .. })
    )
}

// ---------------------------------------------------------------------------
// Meta helpers
// ---------------------------------------------------------------------------

/// Finds a working alternate Stremio type for `meta_id` by consulting the
/// addon's own manifest, for the case where `tried_type` (derived generically
/// from `MediaKind`) 404s. Only considers `meta` resources whose `idPrefixes`
/// match `meta_id` (or which have no prefix restriction), and probes each
/// candidate type with a real request — the manifest can list types that
/// don't apply to this specific ID, so a match here isn't guaranteed either.
/// Returns the successful fetch directly to avoid a redundant second request.
async fn manifest_meta_type_fallback(
    svc: &stremio_service::StremioService,
    tried_type: &sdks::stremio::MediaType,
    meta_id: &str,
) -> Option<sdks::stremio::Meta> {
    let manifest = svc
        .get_manifest()
        .await
        .ok()?;
    let tried = tried_type.to_string();
    let candidates: Vec<String> = manifest
        .resources
        .into_iter()
        .filter_map(|r| match r {
            sdks::stremio::Resource::Detailed(r) if r.name == ResourceType::Meta => {
                let prefix_ok = r
                    .id_prefixes
                    .as_ref()
                    .map(|prefixes| {
                        prefixes
                            .iter()
                            .any(|p| meta_id.starts_with(p.as_str()))
                    })
                    .unwrap_or(true);
                prefix_ok.then_some(r.types)
            }
            _ => None,
        })
        .flatten()
        .filter(|t| t != &tried)
        .collect();

    for candidate in candidates {
        let alt_type = sdks::stremio::MediaType::Other(candidate);
        if let Ok(meta) = svc
            .get_meta(alt_type, meta_id.to_string())
            .await
        {
            return Some(meta);
        }
    }
    None
}

/// Fetch the raw Stremio `Meta` for `media`, storing it in `cache` keyed by
/// the series-level lookup id. Returns the cached `Arc` immediately if present.
async fn fetch_and_cache_meta(
    svc: &stremio_service::StremioService,
    media: &db::Media,
    cache: &std::sync::Mutex<
        std::collections::HashMap<String, Arc<sdks::stremio::Meta>>,
    >,
    ctx: &AppContext,
) -> Result<Arc<sdks::stremio::Meta>> {
    // For Season/Episode, the series meta is cached under the series' ID.
    // Prefer grandparent lookup so Seasons/Episodes with empty own external_ids
    // still resolve to the correct cache entry.
    let meta_id: String = media
        .grandparent
        .as_deref()
        .and_then(|gp| {
            gp.external_ids
                .stremio_lookup_id()
        })
        .or_else(|| {
            media
                .external_ids
                .stremio_lookup_id()
        })
        .ok_or_else(|| anyhow!("no resolvable meta id for {}", media.id))?;

    if let Some(cached) = cache
        .lock()
        .unwrap()
        .get(&meta_id)
        .cloned()
    {
        return Ok(cached);
    }

    let series_imdb = media
        .grandparent
        .as_deref()
        .and_then(|gp| {
            gp.external_ids
                .imdb
                .clone()
        });
    let is_custom = media
        .external_ids
        .imdb
        .is_none()
        && series_imdb.is_none();
    let media_type = media
        .external_ids
        .stremio_media_type(&media.kind);
    let meta: Arc<sdks::stremio::Meta> = if let Some(stored) = ctx
        .store
        .get::<sdks::stremio::Meta>(
            media
                .id
                .to_string(),
        ) {
        stored
    } else {
        Arc::new(
            match svc
                .get_meta(media_type.clone(), meta_id.clone())
                .await
            {
                Ok(m) => m,
                // Custom-ID items (no IMDB/TMDB) have no `MediaKind`-derived type that's
                // guaranteed correct: a DB row imported before `custom_stremio_type` was
                // tracked (or a season/episode that never inherited it) falls back to a
                // generic type that may not match the addon's own non-standard one (e.g.
                // "anime"). Ask the addon's manifest what type(s) it actually serves for
                // this ID and retry, rather than failing permanently.
                Err(e)
                    if is_404(&e)
                        && is_custom
                        && media
                            .external_ids
                            .custom_stremio_type
                            .is_none() =>
                {
                    match manifest_meta_type_fallback(svc, &media_type, &meta_id).await
                    {
                        Some(m) => m,
                        None => return Err(e),
                    }
                }
                Err(e) if is_404(&e) && !is_custom => {
                    let series_tmdb = media
                        .grandparent
                        .as_deref()
                        .and_then(|gp| {
                            gp.external_ids
                                .tmdb
                        });
                    let tmdb_id = media
                        .external_ids
                        .tmdb
                        .or(series_tmdb);
                    if let Some(tid) = tmdb_id {
                        svc.get_meta(media_type, format!("tmdb:{}", tid))
                            .await?
                    } else {
                        return Err(e);
                    }
                }
                Err(e) => return Err(e),
            },
        )
    };

    let arc = meta;
    cache
        .lock()
        .unwrap()
        .insert(meta_id, Arc::clone(&arc));
    Ok(arc)
}

async fn stremio_meta_fetch(
    svc: &stremio_service::StremioService,
    media: &db::Media,
    ctx: &AppContext,
    medias_cache: &std::sync::Mutex<
        std::collections::HashMap<String, Arc<sdks::stremio::Meta>>,
    >,
) -> Result<Option<db::Media>> {
    let imdb_id = media
        .grandparent
        .as_deref()
        .and_then(|gp| {
            gp.external_ids
                .imdb
                .clone()
        })
        .or(media
            .external_ids
            .imdb
            .clone());
    let is_custom = imdb_id.is_none();

    let meta_arc = fetch_and_cache_meta(svc, media, medias_cache, ctx).await?;

    match media.kind {
        db::MediaKind::Movie | db::MediaKind::Series => {
            // Patch imdb_id into a mutable clone for root-level conversion and
            // relations. Only the Movie/Series arm needs the owned copy — cloning
            // it unconditionally deep-copies every entry in `videos`, which is
            // ruinous for series with thousands of episodes.
            let mut meta_patched = (*meta_arc).clone();
            if meta_patched
                .imdb_id
                .is_none()
                && !is_custom
            {
                meta_patched.imdb_id =
                    db::ExternalIds::from_stremio_id(&meta_patched.id)
                        .imdb
                        .map(Into::into)
                        .or_else(|| imdb_id.map(Into::into));
            }
            if meta_patched.is_error() {
                warn!(
                    id = %media.id,
                    error_title = %meta_patched.get_name().unwrap_or_default(),
                    error_description = %meta_patched.description.as_deref().unwrap_or(""),
                    "meta addon returned an error, skipping"
                );
                return Ok(None);
            }
            let mut found =
                db::Media::try_from(meta_patched.clone()).map_err(|e| anyhow!(e))?;
            // Preserve the persisted ID — try_from recomputes it from external_ids.
            found.id = media.id;
            let relations = build_relations(media, &meta_patched);
            if !relations.is_empty() {
                found.relations = Some(relations);
            }
            Ok(Some(found))
        }
        db::MediaKind::Season => {
            let series_id = media
                .grandparent_id
                .or(media.parent_id)
                .ok_or_else(|| anyhow!("season {} missing grandparent_id", media.id))?;
            let series_external_ids = media
                .grandparent
                .as_deref()
                .map(|gp| {
                    gp.external_ids
                        .clone()
                })
                .ok_or_else(|| anyhow!("season {} missing grandparent", media.id))?;
            let seasons =
                db::stremio_meta_seasons(&meta_arc, series_id, &series_external_ids);
            Ok(seasons
                .into_iter()
                .find(|s| s.idx == media.idx))
        }
        db::MediaKind::Episode => {
            let series_id = media
                .grandparent_id
                .ok_or_else(|| {
                    anyhow!("episode {} missing grandparent_id", media.id)
                })?;
            let season_id = media
                .parent_id
                .ok_or_else(|| anyhow!("episode {} missing parent_id", media.id))?;
            let season_idx = media
                .parent_idx
                .ok_or_else(|| anyhow!("episode {} missing parent_idx", media.id))?;
            let series_external_ids = media
                .grandparent
                .as_deref()
                .map(|gp| {
                    gp.external_ids
                        .clone()
                })
                .ok_or_else(|| anyhow!("episode {} missing grandparent", media.id))?;
            // Locate just this episode's video entry. Materialising the whole
            // season here and discarding all but one row made a season of N
            // episodes cost O(N^2) to refresh.
            let Some(meta_ep) = meta_arc
                .videos
                .as_ref()
                .and_then(|v| {
                    v.iter()
                        .find(|e| {
                            e.episode == media.idx && e.season == media.parent_idx
                        })
                })
            else {
                return Ok(None);
            };
            let mut found = db::stremio_meta_episode(
                meta_ep,
                series_id,
                season_id,
                season_idx,
                &series_external_ids,
            )?;
            let relations = build_episode_relations(media, meta_ep);
            if !relations.is_empty() {
                found.relations = Some(relations);
            }
            Ok(Some(found))
        }
        _ => Ok(None),
    }
}

// ---------------------------------------------------------------------------
// Relation builders
// ---------------------------------------------------------------------------

pub(crate) fn build_relations(
    media: &db::Media,
    meta: &sdks::stremio::Meta,
) -> Vec<(db::MediaRelation, db::Media)> {
    let mut relations = Vec::new();

    if let Some(genres) = meta
        .genre
        .as_ref()
        .or(meta
            .genres
            .as_ref())
    {
        for genre_name in genres {
            let genre_id = common::stable_media_uuid(
                &db::MediaKind::Genre,
                &genre_name.to_lowercase(),
            );
            relations.push((
                db::MediaRelation {
                    left_media_id: media.id,
                    right_media_id: genre_id,
                    role: None,
                    ..Default::default()
                },
                db::Media {
                    id: genre_id,
                    title: genre_name.clone(),
                    kind: db::MediaKind::Genre,
                    ..Default::default()
                },
            ));
        }
    }

    let mut rels = build_person_relations(
        media.id,
        meta.director
            .as_ref(),
        meta.writer
            .as_ref(),
        None,
        meta.cast
            .as_ref(),
        None,
        None,
    );

    if let Some(extras) = &meta.app_extras {
        rels.extend(build_person_relations(
            media.id,
            None,
            None,
            extras
                .cast
                .as_ref(),
            None,
            extras
                .directors
                .as_ref(),
            extras
                .writers
                .as_ref(),
        ));
    }

    relations.extend(rels);
    relations
}

pub(crate) fn build_episode_relations(
    media: &db::Media,
    ep: &sdks::stremio::Episode,
) -> Vec<(db::MediaRelation, db::Media)> {
    build_person_relations(
        media.id,
        ep.directors
            .as_ref(),
        ep.writers
            .as_ref(),
        None,
        None,
        None,
        None,
    )
}

fn build_person_relations(
    left_media_id: Uuid,
    directors: Option<&Vec<String>>,
    writers: Option<&Vec<String>>,
    cast_members: Option<&Vec<sdks::stremio::CastMember>>,
    cast_names: Option<&Vec<String>>,
    director_members: Option<&Vec<sdks::stremio::CastMember>>,
    writer_members: Option<&Vec<sdks::stremio::CastMember>>,
) -> Vec<(db::MediaRelation, db::Media)> {
    let mut relations = Vec::new();

    let split_names = |names: Option<&Vec<String>>| -> Vec<String> {
        names
            .map(|v| v.as_slice())
            .unwrap_or_default()
            .iter()
            .flat_map(|s| {
                s.split(',')
                    .map(|n| {
                        n.trim()
                            .to_string()
                    })
            })
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut add_members = |members: Option<&Vec<sdks::stremio::CastMember>>,
                           role: db::RelationRole,
                           offset: i64| {
        if let Some(list) = members {
            for (i, member) in list
                .iter()
                .enumerate()
            {
                if let Some(name) = &member.name {
                    let name = name
                        .trim()
                        .to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let person_id = common::stable_media_uuid(
                        &db::MediaKind::Person,
                        &name.to_lowercase(),
                    );
                    let mut person = db::Media {
                        id: person_id,
                        title: name.clone(),
                        kind: db::MediaKind::Person,
                        ..Default::default()
                    };
                    if let Some(url) = member
                        .photo
                        .clone()
                    {
                        person.set_image(db::ImageKind::Primary, url);
                    }
                    relations.push((
                        db::MediaRelation {
                            left_media_id,
                            right_media_id: person_id,
                            weight: Some(offset + i as i64),
                            role: Some(role.clone()),
                            character: member
                                .character
                                .clone(),
                            ..Default::default()
                        },
                        person,
                    ));
                }
            }
        }
    };

    add_members(cast_members, db::RelationRole::Actor, 0);
    add_members(director_members, db::RelationRole::Director, 0);
    add_members(writer_members, db::RelationRole::Writer, 0);

    for (i, name) in split_names(cast_names)
        .into_iter()
        .enumerate()
    {
        let person_id =
            common::stable_media_uuid(&db::MediaKind::Person, &name.to_lowercase());
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(
                    (i + cast_members
                        .map(|c| c.len())
                        .unwrap_or(0)) as i64,
                ),
                role: Some(db::RelationRole::Actor),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                ..Default::default()
            },
        ));
    }

    for (i, name) in split_names(directors)
        .into_iter()
        .enumerate()
    {
        let person_id =
            common::stable_media_uuid(&db::MediaKind::Person, &name.to_lowercase());
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(
                    (i + director_members
                        .map(|c| c.len())
                        .unwrap_or(0)) as i64,
                ),
                role: Some(db::RelationRole::Director),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                ..Default::default()
            },
        ));
    }

    for (i, name) in split_names(writers)
        .into_iter()
        .enumerate()
    {
        let person_id =
            common::stable_media_uuid(&db::MediaKind::Person, &name.to_lowercase());
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(
                    (i + writer_members
                        .map(|c| c.len())
                        .unwrap_or(0)) as i64,
                ),
                role: Some(db::RelationRole::Writer),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                ..Default::default()
            },
        ));
    }

    relations
}

// ---------------------------------------------------------------------------
// Search helpers
// ---------------------------------------------------------------------------

async fn stremio_search(
    svc: &stremio_service::StremioService,
    kind: &db::MediaKind,
    query: &str,
    limit: usize,
    ctx: &AppContext,
) -> Result<Vec<db::Media>> {
    use itertools::Itertools;

    let aio_type = match kind {
        db::MediaKind::Movie => sdks::stremio::MediaType::Movie,
        db::MediaKind::Series => sdks::stremio::MediaType::Series,
        _ => return Ok(vec![]),
    };

    let results = svc
        .search(aio_type, query.to_string())
        .await
        .unwrap_or_default();

    let mut media = results
        .into_iter()
        .unique_by(|m| {
            m.imdb_id
                .as_ref()
                .filter(|id| !id.is_empty())
                .map(|id| format!("imdb:{}", id))
                .unwrap_or_else(|| format!("{}:{}", m.media_type, m.id))
        })
        .take(limit)
        .filter(|meta| !meta.is_error())
        .filter_map(|meta| {
            let mut m = db::Media::try_from(meta.clone()).ok()?;
            let rels = build_relations(&m, &meta);
            m.relations = Some(rels);
            Some(m)
        })
        .collect();

    db::Media::preload_parents(&ctx.db, &mut media).await;

    Ok(media)
}

// ---------------------------------------------------------------------------
// Subtitle helpers
// ---------------------------------------------------------------------------

async fn stremio_subtitles(
    svc: &stremio_service::StremioService,
    media: &db::Media,
) -> Result<Vec<sdks::stremio::Subtitle>> {
    let (imdb_id, media_type, season, episode) = match media.kind {
        db::MediaKind::Movie => (
            media
                .external_ids
                .imdb
                .as_deref()
                .ok_or_else(|| anyhow!("no imdb_id"))?,
            sdks::stremio::MediaType::Movie,
            None,
            None,
        ),
        db::MediaKind::Episode => (
            media
                .grandparent
                .as_deref()
                .and_then(|gp| {
                    gp.external_ids
                        .imdb
                        .as_deref()
                })
                .ok_or_else(|| anyhow!("no grandparent imdb for subtitle lookup"))?,
            sdks::stremio::MediaType::Series,
            media.parent_idx,
            media.idx,
        ),
        _ => return Err(anyhow!("subtitles not supported for {:?}", media.kind)),
    };

    svc.get_subtitles(media_type, imdb_id, season, episode)
        .await
}

// ---------------------------------------------------------------------------
// Stream helpers
// ---------------------------------------------------------------------------

/// Rewrite a URL whose host is `aiostreams` to use the stremio addon's origin.
/// AIO running in Docker uses this internal hostname; we remap it at descriptor
/// construction time so callers never see the unresolvable internal address.
/// Extract tracker URLs from a Stremio stream's `sources` array.
///
/// Tracker entries are conventionally `"tracker:udp://…/announce"`, but some
/// addons (notably private-tracker addons) emit the bare URL without the
/// `tracker:` prefix. Accept both and run each through [`TrackerUrl`]
/// validation so unrelated `sources` entries are dropped; de-duplicate.
fn extract_trackers(sources: &[String]) -> Vec<crate::stream::TrackerUrl> {
    let mut seen = std::collections::HashSet::new();
    let mut trackers = Vec::new();
    for src in sources {
        let url = src
            .strip_prefix("tracker:")
            .unwrap_or(src.as_str())
            .trim();
        if let Ok(url) = crate::stream::TrackerUrl::try_new(url.to_string()) {
            if seen.insert(url.clone()) {
                trackers.push(url);
            }
        }
    }
    trackers
}

struct StremioStreamMetadata {
    filename: Option<String>,
    file_idx: Option<usize>,
    seeders: Option<i64>,
}

fn stremio_stream_metadata(stream: &sdks::stremio::Stream) -> StremioStreamMetadata {
    let stream_data = stream
        .stream_data
        .as_ref();
    let torrent = stream_data.and_then(|data| {
        data.torrent
            .as_ref()
    });

    StremioStreamMetadata {
        filename: stream
            .behavior_hints
            .as_ref()
            .and_then(|hints| {
                hints
                    .filename
                    .clone()
            })
            .or_else(|| {
                stream_data.and_then(|data| {
                    data.filename
                        .clone()
                })
            })
            .or_else(|| {
                stream
                    .filename
                    .clone()
            }),
        file_idx: stream
            .file_idx
            .and_then(|index| usize::try_from(index).ok())
            .or_else(|| {
                torrent
                    .and_then(|torrent| torrent.file_idx)
                    .and_then(|index| usize::try_from(index).ok())
            }),
        seeders: stream
            .seeders
            .or_else(|| torrent.and_then(|torrent| torrent.seeders)),
    }
}

fn rewrite_aio_url(url: &str, manifest_url: &StremioManifestUrl) -> String {
    let Ok(mut parsed) = url::Url::parse(url) else {
        return url.to_string();
    };
    if !parsed
        .host_str()
        .map(|h| h.eq_ignore_ascii_case("aiostreams"))
        .unwrap_or(false)
    {
        return url.to_string();
    }
    let Ok(origin) = url::Url::parse(manifest_url.as_str()) else {
        return url.to_string();
    };
    let _ = parsed.set_scheme(origin.scheme());
    let _ = parsed.set_host(origin.host_str());
    let _ = parsed.set_port(origin.port());
    parsed.to_string()
}

fn stream_identifying_text(s: &sdks::stremio::Stream) -> String {
    let mut parts = Vec::new();
    let meta = stremio_stream_metadata(s);
    if let Some(ref fn_) = meta.filename {
        parts.push(fn_.as_str());
    }
    if let Some(ref folder) = s.folder_name {
        parts.push(folder.as_str());
    }
    if let Some(ref desc) = s.description {
        parts.push(desc.as_str());
    }
    if let Some(ref name) = s.name {
        parts.push(name.as_str());
    }
    parts.join(" ")
}

fn extract_year(text: &str) -> Option<u32> {
    static YEAR_RE: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| {
            regex::Regex::new(r"(?i)(?:^|[\s._\-\[(])(19\d\d|20\d\d)(?:[\s._\-\])]|$)")
                .unwrap()
        });
    for cap in YEAR_RE.captures_iter(text) {
        if let Some(m) = cap.get(1) {
            if let Ok(y) = m
                .as_str()
                .parse::<u32>()
            {
                if (1920..=2040).contains(&y) && y != 1080 && y != 2160 {
                    return Some(y);
                }
            }
        }
    }
    None
}

fn stremio_stream_matches_media(s: &sdks::stremio::Stream, media: &db::Media) -> bool {
    let meta = stremio_stream_metadata(s);
    let filename = meta
        .filename
        .as_deref()
        .unwrap_or_default();

    if let Some(ext) = filename
        .rsplit('.')
        .next()
    {
        if matches!(
            ext.to_ascii_lowercase()
                .as_str(),
            "z01"
                | "z02"
                | "zip"
                | "rar"
                | "r00"
                | "r01"
                | "7z"
                | "tar"
                | "gz"
                | "txt"
                | "nfo"
                | "exe"
        ) {
            return false;
        }
    }

    let text = stream_identifying_text(s);
    if text
        .trim()
        .is_empty()
    {
        return true;
    }

    // Release year mismatch check (e.g. 1999 anime vs 2023 live action remake)
    let target_year: Option<u32> = match media.kind {
        db::MediaKind::Episode => media
            .grandparent
            .as_ref()
            .and_then(|gp| gp.released_at)
            .or(media.released_at)
            .map(|d| chrono::Datelike::year(&d) as u32),
        _ => media
            .released_at
            .map(|d| chrono::Datelike::year(&d) as u32),
    };
    if let Some(target_y) = target_year {
        if let Some(stream_y) = extract_year(&text) {
            if stream_y != target_y && (stream_y as i32 - target_y as i32).abs() > 1 {
                tracing::debug!(
                    stream_year = stream_y,
                    target_year = target_y,
                    "stremio stream rejected: year mismatch"
                );
                return false;
            }
        }
    }

    match media.kind {
        db::MediaKind::Episode => {
            let series_title = media
                .grandparent
                .as_ref()
                .map(|gp| {
                    gp.title
                        .as_str()
                })
                .unwrap_or(&media.title);
            let target_season = media
                .parent_idx
                .unwrap_or(1) as u32;
            let target_episode = media
                .idx
                .unwrap_or(1) as u32;

            // 1. Series Title verification:
            let series_tokens = crate::common::significant_tokens(series_title);
            let ep_tokens = crate::common::significant_tokens(&media.title);
            let norm_text = crate::common::normalize_for_match(&text);

            let matches_title = (series_tokens.is_empty() && ep_tokens.is_empty())
                || crate::common::contains_all_tokens(&norm_text, &series_tokens)
                || crate::common::contains_all_tokens(&norm_text, &ep_tokens);
            if !matches_title {
                tracing::debug!(
                    stream_text = %text,
                    %series_title,
                    "stremio stream rejected: does not match series title"
                );
                return false;
            }

            // 2. Episode verification:
            // If the stream references a specific episode in its filename or descriptor,
            // verify it matches the target episode.
            if !filename.is_empty() {
                let wanted_se = format!("S{:02}E{:02}", target_season, target_episode);
                if let Some((parsed_season, parsed_ep)) =
                    crate::torrent::parse_season_episode(filename)
                {
                    if let Some(s) = parsed_season {
                        if s != target_season {
                            tracing::debug!(
                                %filename,
                                target_season,
                                parsed_season = s,
                                "stremio stream rejected: season mismatch"
                            );
                            return false;
                        }
                    }
                    if parsed_ep != target_episode
                        && !crate::torrent::matches_episode_pattern(
                            filename, &wanted_se,
                        )
                    {
                        tracing::debug!(
                            %filename,
                            target_episode,
                            parsed_episode = parsed_ep,
                            "stremio stream rejected: episode mismatch"
                        );
                        return false;
                    }
                }
            }

            true
        }
        db::MediaKind::Movie => {
            let movie_tokens = crate::common::significant_tokens(&media.title);
            let norm_text = crate::common::normalize_for_match(&text);
            if !crate::common::contains_all_tokens(&norm_text, &movie_tokens) {
                tracing::debug!(
                    stream_text = %text,
                    movie_title = %media.title,
                    "stremio stream rejected: does not match movie title"
                );
                return false;
            }
            true
        }
        _ => true,
    }
}

async fn stremio_streams(
    svc: &stremio_service::StremioService,
    manifest_url: &StremioManifestUrl,
    media: &db::Media,
    id_prefixes: Option<&[String]>,
    strict_stream_matching: bool,
    resolve_debrid_packs_as_torrents: bool,
) -> Result<Vec<crate::stream::StreamInfo>> {
    let gp_ext = media
        .grandparent
        .as_deref()
        .map(|gp| &gp.external_ids);
    let all_candidates = media.candidate_ids(gp_ext);
    let ids_to_try: Vec<String> = match id_prefixes {
        Some(prefixes) => all_candidates
            .into_iter()
            .filter(|id| {
                prefixes
                    .iter()
                    .any(|p| id.starts_with(p.as_str()))
            })
            .collect(),
        None => all_candidates,
    };
    if ids_to_try.is_empty() {
        return Err(anyhow!("no resolvable ID for Stremio stream lookup"));
    }
    let media_type = media
        .external_ids
        .stremio_media_type(&media.kind);

    let mut last_err: Option<anyhow::Error> = None;
    let mut valid_streams: Vec<sdks::stremio::Stream> = Vec::new();
    let mut seen_keys = std::collections::HashSet::new();

    for id in ids_to_try {
        match svc
            .get_streams(media_type.clone(), id)
            .await
        {
            Ok(s) => {
                for stream in s {
                    if stream.is_valid()
                        && (!strict_stream_matching
                            || stremio_stream_matches_media(&stream, media))
                    {
                        let metadata = stremio_stream_metadata(&stream);
                        let filename = metadata
                            .filename
                            .as_deref()
                            .unwrap_or_default()
                            .trim()
                            .to_ascii_lowercase();
                        let dedup_key = if let Some(url) = stream
                            .url
                            .as_deref()
                            .or(stream
                                .external_url
                                .as_deref())
                        {
                            format!("http:{}", url.trim())
                        } else if let Some(info_hash) = stream
                            .info_hash
                            .as_deref()
                        {
                            let file = metadata
                                .file_idx
                                .map(|index| format!("#{index}"))
                                .unwrap_or(filename);
                            format!("torrent:{}:{file}", info_hash.to_ascii_lowercase())
                        } else {
                            filename
                        };
                        if dedup_key.is_empty() || seen_keys.insert(dedup_key) {
                            valid_streams.push(stream);
                        }
                    }
                }
            }
            Err(e) if is_404(&e) => {
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    if valid_streams.is_empty() {
        return Err(last_err.unwrap_or_else(|| anyhow!("no streams found")));
    }

    Ok(valid_streams
        .into_iter()
        .filter_map(|s| {
            let sd = s
                .stream_data
                .as_ref();
            let metadata = stremio_stream_metadata(&s);
            let unresolved_pack_hash = if resolve_debrid_packs_as_torrents
                && media.kind == db::MediaKind::Episode
            {
                sd.and_then(|data| {
                    data.torrent
                        .as_ref()
                })
                .filter(|torrent| {
                    torrent
                        .file_idx
                        .is_some_and(|index| index < 0)
                })
                .and_then(|torrent| {
                    torrent
                        .info_hash
                        .as_deref()
                })
                .filter(|hash| {
                    hash.len() == 40
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_hexdigit())
                })
            } else {
                None
            };
            let descriptor = if s.is_torrent() || unresolved_pack_hash.is_some() {
                let trackers = extract_trackers(
                    s.sources
                        .as_deref()
                        .unwrap_or_default(),
                );
                debug!(
                    info_hash = ?s.info_hash(),
                    ?trackers,
                    "torrent stream trackers"
                );
                crate::stream::StreamDescriptor::Torrent {
                    info_hash: s
                        .info_hash()
                        .or(unresolved_pack_hash)?
                        .to_ascii_lowercase(),
                    file_hint: if unresolved_pack_hash.is_some() {
                        let series_title = media
                            .grandparent
                            .as_deref()
                            .map(|series| {
                                series
                                    .title
                                    .as_str()
                            })
                            .unwrap_or(
                                media
                                    .title
                                    .as_str(),
                            );
                        Some(format!(
                            "{series_title} S{:02}E{:02}",
                            media
                                .parent_idx
                                .unwrap_or(1),
                            media
                                .idx
                                .unwrap_or(1)
                        ))
                    } else {
                        metadata
                            .filename
                            .clone()
                    },
                    file_idx: if unresolved_pack_hash.is_some() {
                        None
                    } else {
                        metadata.file_idx
                    },
                    trackers,
                }
            } else {
                let url = s
                    .url
                    .clone()
                    .or_else(|| {
                        s.external_url
                            .clone()
                    })?;
                crate::stream::StreamDescriptor::Http {
                    url: rewrite_aio_url(&url, manifest_url),
                    request_headers: s
                        .request_headers
                        .clone(),
                    response_headers: s
                        .response_headers
                        .clone(),
                }
            };
            let label = match (
                s.name
                    .as_deref(),
                s.description
                    .as_deref(),
            ) {
                (Some(n), Some(d)) if !d.is_empty() => format!("{}\n{}", n, d),
                (Some(n), _) => n.to_string(),
                (None, Some(d)) => d.to_string(),
                _ => "Stream".to_string(),
            };
            // Prefer nzb_url from streamData (AIOStreams), fall back to top-level field
            let nzb_url = sd
                .and_then(|d| {
                    d.nzb_url
                        .clone()
                })
                .or_else(|| {
                    s.nzb_url
                        .clone()
                });
            let usenet_guid = nzb_url
                .as_deref()
                .and_then(|u| {
                    url::Url::parse(u)
                        .ok()?
                        .query_pairs()
                        .find_map(|(k, v)| (k == "id").then(|| v.into_owned()))
                });
            let torrent_info_hash = sd
                .and_then(|d| {
                    d.torrent
                        .as_ref()
                })
                .and_then(|t| {
                    t.info_hash
                        .as_deref()
                })
                .map(|h| h.to_ascii_lowercase());
            let torrent_file_idx = sd
                .and_then(|d| {
                    d.torrent
                        .as_ref()
                })
                .and_then(|t| t.file_idx)
                .filter(|&i| i >= 0);
            Some(crate::stream::StreamInfo {
                descriptor,
                name: Some(label),
                description: s
                    .description
                    .clone(),
                filename: metadata.filename,
                seeders: metadata.seeders,
                size: sd
                    .and_then(|d| d.size)
                    .or(s.size),
                duration: s.duration,
                subtitles: s
                    .subtitles
                    .clone(),
                binge_group: s
                    .behavior_hints
                    .as_ref()
                    .and_then(|bh| {
                        bh.binge_group
                            .clone()
                    }),
                usenet_guid,
                usenet_indexer: sd
                    .and_then(|d| {
                        d.indexer
                            .clone()
                    })
                    .or_else(|| {
                        s.indexer
                            .clone()
                    }),
                nzb_url,
                torrent_info_hash,
                torrent_file_idx,
                service_id: sd
                    .and_then(|d| {
                        d.service
                            .as_ref()
                    })
                    .and_then(|s| {
                        s.id.as_deref()
                    })
                    .map(|s| s.to_lowercase()),
                probe_data: s
                    .behavior_hints
                    .as_ref()
                    .and_then(|bh| {
                        bh.media_info
                            .as_ref()
                    })
                    .map(remux_sdks::remux::MediaSourceInfo::from),
                ..Default::default()
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stremio_torrent_metadata_uses_nested_fallbacks() {
        let stream: sdks::stremio::Stream = serde_json::from_value(serde_json::json!({
            "infoHash": "0123456789abcdef0123456789abcdef01234567",
            "streamData": {
                "filename": "Bundle/Movie.mkv",
                "torrent": { "fileIdx": 3, "seeders": 42 }
            }
        }))
        .unwrap();

        let metadata = stremio_stream_metadata(&stream);
        assert_eq!(
            metadata
                .filename
                .as_deref(),
            Some("Bundle/Movie.mkv")
        );
        assert_eq!(metadata.seeders, Some(42));
        assert_eq!(metadata.file_idx, Some(3));
    }

    #[test]
    fn stremio_torrent_metadata_prefers_top_level_fields() {
        let stream: sdks::stremio::Stream = serde_json::from_value(serde_json::json!({
            "infoHash": "0123456789abcdef0123456789abcdef01234567",
            "filename": "Top Level.mkv",
            "fileIdx": 7,
            "seeders": 84,
            "behaviorHints": { "filename": "Preferred/Movie.mkv" },
            "streamData": {
                "filename": "Nested/Movie.mkv",
                "torrent": { "fileIdx": 3, "seeders": 42 }
            }
        }))
        .unwrap();

        let metadata = stremio_stream_metadata(&stream);
        assert_eq!(
            metadata
                .filename
                .as_deref(),
            Some("Preferred/Movie.mkv")
        );
        assert_eq!(metadata.seeders, Some(84));
        assert_eq!(metadata.file_idx, Some(7));
    }

    #[test]
    fn manifest_url_strips_manifest_json_with_query_string() {
        let url = StremioManifestUrl::try_new(
            "https://example.com/path/manifest.json?apikey=abc",
        )
        .unwrap();
        assert!(
            !url.as_str()
                .contains("manifest.json"),
            "manifest.json not stripped: {url}"
        );
        assert!(
            url.as_str()
                .contains("apikey=abc"),
            "query string lost: {url}"
        );
    }

    #[test]
    fn manifest_url_strips_manifest_json_without_query_string() {
        let url = StremioManifestUrl::try_new("https://example.com/path/manifest.json")
            .unwrap();
        assert!(
            !url.as_str()
                .contains("manifest.json"),
            "manifest.json not stripped: {url}"
        );
    }

    fn mock_manifest(server: &httpmock::MockServer) {
        server.mock(|when, then| {
            when.path("/manifest.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "id": "fankai-test",
                    "name": "Fankai",
                    "version": "1.0.0",
                    "resources": [
                        "catalog",
                        {"name": "meta", "types": ["anime"], "idPrefixes": ["fk"]}
                    ],
                    "types": ["anime"],
                    "catalogs": []
                }));
        });
    }

    #[tokio::test]
    async fn manifest_meta_type_fallback_retries_with_addon_declared_type() {
        let server = httpmock::MockServer::start();
        mock_manifest(&server);
        let series_attempt = server.mock(|when, then| {
            when.path("/meta/series/fk:27.json");
            then.status(404);
        });
        let anime_attempt = server.mock(|when, then| {
            when.path("/meta/anime/fk:27.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "meta": {"id": "fk:27", "type": "anime", "name": "Bleach Yabai"}
                }));
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();

        // Confirm the generic type really does 404 first.
        let direct = svc
            .get_meta(sdks::stremio::MediaType::Series, "fk:27".to_string())
            .await;
        assert!(direct.is_err());
        series_attempt.assert();

        let meta = manifest_meta_type_fallback(
            &svc,
            &sdks::stremio::MediaType::Series,
            "fk:27",
        )
        .await;

        assert!(
            meta.is_some(),
            "fallback must find the addon's declared \"anime\" type"
        );
        assert_eq!(
            meta.unwrap()
                .get_name(),
            Some("Bleach Yabai".to_string())
        );
        anime_attempt.assert();
    }

    #[tokio::test]
    async fn manifest_meta_type_fallback_none_when_no_type_works() {
        let server = httpmock::MockServer::start();
        mock_manifest(&server);
        server.mock(|when, then| {
            when.path("/meta/anime/fk:999.json");
            then.status(404);
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();
        let meta = manifest_meta_type_fallback(
            &svc,
            &sdks::stremio::MediaType::Series,
            "fk:999",
        )
        .await;

        assert!(meta.is_none());
    }

    #[tokio::test]
    async fn manifest_meta_type_fallback_skips_non_matching_prefix() {
        let server = httpmock::MockServer::start();
        mock_manifest(&server);
        // "tt" is not covered by fankai's declared idPrefixes (["fk"]) — the
        // fallback must not even attempt the "anime" type for it.
        let anime_attempt = server.mock(|when, then| {
            when.path("/meta/anime/tt1234567.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "meta": {"id": "tt1234567", "type": "anime", "name": "Should Not Match"}
                }));
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();
        let meta = manifest_meta_type_fallback(
            &svc,
            &sdks::stremio::MediaType::Series,
            "tt1234567",
        )
        .await;

        assert!(meta.is_none());
        anime_attempt.assert_hits(0);
    }

    fn episode_media(external_ids: db::ExternalIds) -> db::Media {
        db::Media {
            kind: db::MediaKind::Episode,
            parent_idx: Some(1),
            idx: Some(1),
            external_ids,
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn stremio_streams_prefers_captured_video_id_over_reconstruction() {
        let server = httpmock::MockServer::start();
        let reconstructed = server.mock(|when, then| {
            when.path("/stream/anime/fk:27:1:1.json");
            then.status(404);
        });
        let captured = server.mock(|when, then| {
            when.path("/stream/anime/fk-ep-1.json");
            then.status(200)
                .json_body(serde_json::json!({"streams": [{"url": "https://example.com/1.mp4"}]}));
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();
        let manifest_url = StremioManifestUrl::try_new(server.base_url()).unwrap();
        let grandparent = db::Media {
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                custom_stremio_id: Some("fk:27".to_string()),
                custom_stremio_type: Some("anime".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut media = episode_media(db::ExternalIds {
            custom_stremio_id: Some("fk-ep-1".to_string()),
            custom_stremio_type: Some("anime".to_string()),
            ..Default::default()
        });
        media.grandparent = Some(Box::new(grandparent));

        let streams = stremio_streams(&svc, &manifest_url, &media, None, false, false)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        captured.assert();
        reconstructed.assert_hits(0);
    }

    #[tokio::test]
    async fn stremio_streams_falls_back_to_reconstructed_id_when_uncaptured() {
        let server = httpmock::MockServer::start();
        let reconstructed = server.mock(|when, then| {
            when.path("/stream/anime/fk:27:1:1.json");
            then.status(200)
                .json_body(serde_json::json!({"streams": [{"url": "https://example.com/1.mp4"}]}));
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();
        let manifest_url = StremioManifestUrl::try_new(server.base_url()).unwrap();
        let grandparent = db::Media {
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                custom_stremio_id: Some("fk:27".to_string()),
                custom_stremio_type: Some("anime".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut media = episode_media(db::ExternalIds {
            custom_stremio_type: Some("anime".to_string()),
            ..Default::default()
        });
        media.grandparent = Some(Box::new(grandparent));

        let streams = stremio_streams(&svc, &manifest_url, &media, None, false, false)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        reconstructed.assert();
    }

    #[tokio::test]
    async fn unresolved_debrid_pack_can_be_resolved_as_native_torrent() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/stream/series/tt0388629:1:1.json");
            then.status(200)
                .json_body(serde_json::json!({
                    "streams": [{
                        "url": "https://example.com/debrid/season-pack",
                        "behaviorHints": { "filename": "One Piece Season 01" },
                        "streamData": {
                            "torrent": {
                                "infoHash": "ABCDEF0123456789ABCDEF0123456789ABCDEF01",
                                "fileIdx": -1
                            }
                        }
                    }]
                }));
        });

        let svc =
            stremio_service::StremioService::from_url(&server.base_url()).unwrap();
        let manifest_url = StremioManifestUrl::try_new(server.base_url()).unwrap();
        let grandparent = db::Media {
            title: "One Piece".to_string(),
            kind: db::MediaKind::Series,
            external_ids: db::ExternalIds {
                imdb: Some(
                    db::NonEmptyString::try_new("tt0388629".to_string()).unwrap(),
                ),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut media = episode_media(db::ExternalIds::default());
        media.grandparent = Some(Box::new(grandparent));

        let streams = stremio_streams(&svc, &manifest_url, &media, None, false, true)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        match &streams[0].descriptor {
            crate::stream::StreamDescriptor::Torrent {
                info_hash,
                file_hint,
                file_idx,
                ..
            } => {
                assert_eq!(info_hash, "abcdef0123456789abcdef0123456789abcdef01");
                assert_eq!(file_hint.as_deref(), Some("One Piece S01E01"));
                assert_eq!(*file_idx, None);
            }
            descriptor => panic!("expected torrent descriptor, got {descriptor:?}"),
        }
    }

    #[test]
    fn extract_trackers_accepts_prefixed_bare_and_dedupes() {
        use crate::stream::TrackerUrl;
        let sources = vec![
            "tracker:udp://tracker.opentrackr.org:1337/announce".to_string(),
            "https://private-tracker.example/announce".to_string(),
            "tracker:https://private-tracker.example/announce".to_string(),
            "not-a-tracker".to_string(),
            "udp://open.demonii.com:1337/announce".to_string(),
        ];
        let trackers = extract_trackers(&sources);
        let inner = |t: &TrackerUrl| {
            t.as_ref()
                .to_string()
        };
        assert_eq!(
            trackers
                .iter()
                .map(inner)
                .collect::<Vec<_>>(),
            vec![
                "udp://tracker.opentrackr.org:1337/announce",
                "https://private-tracker.example/announce",
                "udp://open.demonii.com:1337/announce",
            ]
        );
    }

    #[test]
    fn extract_trackers_ignores_non_urls() {
        use crate::stream::TrackerUrl;
        let empty: Vec<TrackerUrl> = Vec::new();
        assert_eq!(extract_trackers(&["foo".to_string()]), empty);
        assert_eq!(
            extract_trackers(&["https://tracker.example".to_string()]),
            Vec::<TrackerUrl>::new()
        );
        assert_eq!(extract_trackers(&[]), Vec::<TrackerUrl>::new());
    }

    #[test]
    fn stremio_stream_matches_media_filters_sopranos_and_wrong_episodes() {
        let grandparent = db::Media {
            title: "One Piece".to_string(),
            kind: db::MediaKind::Series,
            ..Default::default()
        };
        let mut episode = db::Media {
            title: "I'm Luffy! The Man Who Will Become the Pirate King!".to_string(),
            kind: db::MediaKind::Episode,
            parent_idx: Some(1),
            idx: Some(1),
            ..Default::default()
        };
        episode.grandparent = Some(Box::new(grandparent));

        let make_stream =
            |name: &str, desc: &str, filename: &str| -> sdks::stremio::Stream {
                serde_json::from_value(serde_json::json!({
                    "name": name,
                    "description": desc,
                    "behaviorHints": {
                        "filename": filename
                    }
                }))
                .unwrap()
            };

        // Unrelated series (Sopranos) -> must be rejected
        let sopranos_stream = make_stream(
            "Comet",
            "The.Sopranos.S01E01.The.Sopranos.1080p.BluRay.REMUX.mkv",
            "The.Sopranos.S01E01.The.Sopranos.1080p.BluRay.REMUX.mkv",
        );
        assert!(!stremio_stream_matches_media(&sopranos_stream, &episode));

        // Wrong episode of One Piece (508) -> must be rejected
        let wrong_ep_stream = make_stream(
            "SeaDex",
            "One Piece - 508 v2 [F-R].mkv",
            "One Piece - 508 v2 [F-R][64511665].mkv",
        );
        assert!(!stremio_stream_matches_media(&wrong_ep_stream, &episode));

        // Wrong episode of One Piece (352) -> must be rejected
        let wrong_ep_stream2 =
            make_stream("ElfCache", "One Piece 0352.mkv", "One Piece 0352.mkv");
        assert!(!stremio_stream_matches_media(&wrong_ep_stream2, &episode));

        // Archive file (.z01) -> must be rejected
        let archive_stream = make_stream("Comet", "", "One Piece.z01");
        assert!(!stremio_stream_matches_media(&archive_stream, &episode));

        // Target episode (S01E01) -> must match!
        let correct_stream = make_stream(
            "Torrentio",
            "[A&C] One Piece S01E01 (0001) [43DA846C].mkv",
            "[A&C] One Piece S01E01 (0001) (DVD HEVC 480p) [43DA846C].mkv",
        );
        assert!(stremio_stream_matches_media(&correct_stream, &episode));

        // Absolute numbering target episode (0001) -> must match!
        let correct_stream_abs = make_stream(
            "Torrentio",
            "One Piece - 0001 (DVD 540p) [F-R].mkv",
            "One Piece - 0001 (DVD 540p) [F-R][bb5cdb70].mkv",
        );
        assert!(stremio_stream_matches_media(&correct_stream_abs, &episode));

        let unresolved_pack: sdks::stremio::Stream =
            serde_json::from_value(serde_json::json!({
                "url": "https://example.com/season-pack",
                "behaviorHints": { "filename": "One Piece Season 01" },
                "streamData": {
                    "torrent": { "infoHash": "abc", "fileIdx": -1 }
                }
            }))
            .unwrap();
        assert!(stremio_stream_matches_media(&unresolved_pack, &episode));

        let mut tokenless_episode = episode.clone();
        tokenless_episode.title = "A".to_string();
        tokenless_episode
            .grandparent
            .as_mut()
            .unwrap()
            .title = "I".to_string();
        let tokenless_stream = make_stream("Addon", "S01E01", "S01E01.mkv");
        assert!(stremio_stream_matches_media(
            &tokenless_stream,
            &tokenless_episode
        ));
    }
}
