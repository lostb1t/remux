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
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Series,
                MediaKind::TvChannel,
                MediaKind::TvProgram,
            ],
            supported_resources_user: vec![
                ResourceType::Catalog,
                ResourceType::Search,
                ResourceType::Subtitles,
                ResourceType::Stream,
            ],
            supported_types_user: vec![
                MediaKind::Movie,
                MediaKind::Series,
                MediaKind::TvChannel,
                MediaKind::TvProgram,
            ],
            options: vec![AddonOption {
                id: "manifest_url".to_string(),
                name: "Manifest URL".to_string(),
                description: Some("Full URL to the addon's manifest.json".to_string()),
                required: true,
                default: None,
                kind: AddonOptionType::Url,
            }],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
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
        let client = super::make_http_client();
        let addon = Arc::new(StremioAddon {
            addon_id,
            manifest_url,
            client,
            medias_cache: Arc::new(std::sync::Mutex::new(
                std::collections::HashMap::new(),
            )),
            failed: Arc::new(std::sync::Mutex::new(std::collections::HashSet::new())),
            manifest: Default::default(),
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
        // A resource given as a plain string takes the manifest's top-level
        // `types` and `idPrefixes` (Stremio addon SDK, manifest.md). An object
        // resource without `idPrefixes` means all ids.
        let inherit = matches!(res, remux_sdks::stremio::Resource::Simple(_));
        let mut resource_ref = res.into_ref();
        if inherit {
            resource_ref.types = manifest
                .types
                .clone();
            resource_ref.id_prefixes = manifest
                .id_prefixes
                .clone();
        }
        resources.push(resource_ref);
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
    addon_id: Uuid,
    manifest_url: StremioManifestUrl,
    client: reqwest::Client,
    /// Raw Stremio `Meta` cached per series lookup-id for the duration of one tree sync.
    /// Shared between the tree-children path and `stremio_meta_fetch` so the API is
    /// called exactly once per series. Evicted by `on_series_done`.
    medias_cache: Arc<
        std::sync::Mutex<std::collections::HashMap<String, Arc<sdks::stremio::Meta>>>,
    >,
    /// Series-level lookup ids whose fetch already failed during this tree
    /// walk. Without this, every season and episode under a series whose
    /// fetch failed independently retries the identical failing request —
    /// once for the series, then again for every child. Checked alongside
    /// `medias_cache` and evicted at the same point, by `on_series_done`.
    failed: Arc<std::sync::Mutex<std::collections::HashSet<String>>>,
    manifest: super::ManifestCache,
}

impl StremioAddon {
    fn service(&self) -> Result<stremio_service::StremioService> {
        Ok(
            stremio_service::StremioService::from_url(&self.manifest_url)?
                .with_shared_rate_limit(common::addon_rate_limit(self.addon_id)),
        )
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
        let info = self
            .manifest
            .get_or_fetch(|| async {
                let manifest = self
                    .service()?
                    .get_manifest()
                    .await?;
                Ok(parse_manifest_info(&manifest))
            })
            .await?;
        Ok(Some(info))
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
        // Catalog pages are fetched speculatively — up to `page_concurrency`
        // pages start in parallel before the first empty/404 page is even
        // seen, since there's no way to know the true page count up front.
        // `meta_concurrency` bounds a very different thing (how many *items*
        // get enriched concurrently across a whole refresh) and can be set
        // much higher than is sane for this; capped independently so a large
        // `meta_concurrency` can't turn a one-page catalog into a burst of
        // speculative requests (and avoidable 429s) against one addon.
        const MAX_CATALOG_PAGE_CONCURRENCY: usize = 5;
        let page_concurrency = db::Settings::get_config_or_default(&ctx.db)
            .await
            .meta_concurrency
            .max(1)
            .min(MAX_CATALOG_PAGE_CONCURRENCY as i64)
            as usize;

        let stream = svc
            .get_catalog_stream(
                kind.to_string(),
                id.to_string(),
                supports_skip,
                page_concurrency,
            )
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
                            // Only emit top-level content. Seasons and episodes
                            // are populated by sync_tree during RefreshLibrary,
                            // but a scheduled virtual channel's program must be
                            // imported alongside its channel for the live guide.
                            items.retain(|x| {
                                x.parent_id.is_none() || x.kind == db::MediaKind::TvProgram
                            });
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
        stremio_meta_fetch(&svc, media, ctx, &self.medias_cache, &self.failed).await
    }

    fn on_meta_fetch_timeout(&self, media: &db::Media) {
        // `tokio::time::timeout` dropped our in-flight fetch before it could
        // record its own failure (see `fetch_and_cache_meta`) — record it
        // here instead, or every other season/episode of this series will
        // independently retry the identical request that just timed out.
        if let Some(meta_id) = stremio_meta_lookup_id(media) {
            self.failed
                .lock()
                .unwrap()
                .insert(meta_id);
        }
    }

    async fn rate_limit_cooldown(&self) -> std::time::Duration {
        common::addon_rate_limit(self.addon_id)
            .remaining_cooldown()
            .await
    }

    fn on_series_done(&self, meta_id: &str) {
        self.medias_cache
            .lock()
            .unwrap()
            .remove(meta_id);
        self.failed
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
                let meta_arc = fetch_and_cache_meta(
                    &svc,
                    root,
                    &self.medias_cache,
                    &self.failed,
                    ctx,
                )
                .await?;
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

    async fn rate_limit_cooldown(&self) -> std::time::Duration {
        common::addon_rate_limit(self.addon_id)
            .remaining_cooldown()
            .await
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
            .map(|s| {
                use crate::subtitle_selection::{
                    SubtitleMarker, has_hi_marker, has_subtitle_marker,
                };
                let is_forced = has_subtitle_marker(
                    s.subtitle_file_name
                        .as_deref(),
                    SubtitleMarker::Forced,
                ) || has_subtitle_marker(
                    s.title
                        .as_deref(),
                    SubtitleMarker::Forced,
                );
                let is_hi = has_hi_marker(
                    s.subtitle_file_name
                        .as_deref(),
                    s.lang
                        .as_deref(),
                ) || has_hi_marker(
                    s.title
                        .as_deref(),
                    s.lang
                        .as_deref(),
                );
                SubtitleInfo {
                    id: s.id,
                    url: Some(crate::stream::StreamDescriptor::http(s.url)),
                    lang: s.lang,
                    is_forced,
                    is_hi,
                    // Prefer the actual release filename over the addon's
                    // display title: filename is what release-match logic
                    // compares against the video's own filename, and a title
                    // like "English" would otherwise defeat that match.
                    filename: s
                        .subtitle_file_name
                        .or(s.movie_release_name)
                        .or(s.title),
                    from_trusted: s.from_trusted,
                    ai_translated: s.ai_translated,
                }
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
        stremio_streams(&svc, &self.manifest_url, media, id_prefixes).await
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
        db::MediaKind::TvChannel | db::MediaKind::TvProgram => Some("tv"),
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

/// The cache/failure-tracking key for `media`: the series-level lookup id.
/// For Season/Episode, prefer the grandparent so items with empty own
/// external_ids still resolve to the correct series entry. Shared by
/// `fetch_and_cache_meta` and `on_meta_fetch_timeout` so both agree on
/// exactly the same key for the same media.
fn stremio_meta_lookup_id(media: &db::Media) -> Option<String> {
    media
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
}

/// Fetch the raw Stremio `Meta` for `media`, storing it in `cache` keyed by
/// the series-level lookup id. Returns the cached `Arc` immediately if present.
async fn fetch_and_cache_meta(
    svc: &stremio_service::StremioService,
    media: &db::Media,
    cache: &std::sync::Mutex<
        std::collections::HashMap<String, Arc<sdks::stremio::Meta>>,
    >,
    failed: &std::sync::Mutex<std::collections::HashSet<String>>,
    ctx: &AppContext,
) -> Result<Arc<sdks::stremio::Meta>> {
    let meta_id: String = stremio_meta_lookup_id(media)
        .ok_or_else(|| anyhow!("no resolvable meta id for {}", media.id))?;

    if let Some(cached) = cache
        .lock()
        .unwrap()
        .get(&meta_id)
        .cloned()
    {
        return Ok(cached);
    }

    // The series-level fetch for this meta_id already failed once during
    // this tree walk — don't let every other season/episode under it retry
    // the identical failing request. Bail immediately instead.
    if failed
        .lock()
        .unwrap()
        .contains(&meta_id)
    {
        return Err(anyhow!(
            "skipping {meta_id}: already failed earlier in this refresh"
        ));
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
    // Wrapped so every failure path below records into `failed` in one place
    // instead of needing to remember to do it at each `return Err`/`?` site.
    let fetch_result: Result<Arc<sdks::stremio::Meta>> = async {
        if let Some(stored) = ctx
            .store
            .get::<sdks::stremio::Meta>(
                media
                    .id
                    .to_string(),
            )
        {
            return Ok(stored);
        }
        Ok(Arc::new(
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
        ))
    }
    .await;

    let arc = match fetch_result {
        Ok(arc) => arc,
        Err(e) => {
            failed
                .lock()
                .unwrap()
                .insert(meta_id);
            return Err(e);
        }
    };
    // The addon answered 200 OK, but the payload itself signals failure
    // (Stremio addons report upstream errors this way, not via HTTP status —
    // e.g. AIO's own upstream coming back empty). Caching this into `cache`
    // would burn a transient error in for the rest of this series' tree
    // walk: every other season/episode would replay the exact same stale
    // error, even seconds later once the addon has recovered. Route it
    // through the same `failed` short-circuit as a hard failure instead —
    // one real attempt (and one log line) per series per run, not one per
    // episode.
    if arc.is_error() {
        failed
            .lock()
            .unwrap()
            .insert(meta_id.clone());
        return Err(anyhow!(
            "{meta_id} returned an error meta: {}",
            arc.get_name()
                .unwrap_or_default()
        ));
    }
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
    failed: &std::sync::Mutex<std::collections::HashSet<String>>,
) -> Result<Option<db::Media>> {
    // This series already failed once this tree walk. Return quietly — the
    // original failure was already logged; every episode re-surfacing it as
    // a fresh `error!()` just floods the log without telling anyone anything
    // new. `Ok(None)` (not an Err) means "this addon has nothing to add",
    // the same as an addon that never applied to this item at all.
    //
    // A cache hit always wins over this, checked first: a concurrent call
    // for the same series may have already fetched successfully after this
    // one's own failure was recorded (the two can race, since permits are
    // shared across an entire batch, not scoped per series), and skipping a
    // real, cached success because of a marker from an earlier, unrelated
    // failure would be strictly worse than the redundant lock check.
    if let Some(meta_id) = stremio_meta_lookup_id(media) {
        let cached = medias_cache
            .lock()
            .unwrap()
            .contains_key(&meta_id);
        if !cached
            && failed
                .lock()
                .unwrap()
                .contains(&meta_id)
        {
            return Ok(None);
        }
    }

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

    let meta_arc = fetch_and_cache_meta(svc, media, medias_cache, failed, ctx).await?;

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
            // No `is_error()` check needed here: `fetch_and_cache_meta` never
            // returns (or caches) an error-shaped meta — it converts those to
            // an `Err` before either the cache-hit or fresh-fetch path can
            // hand one back, so `meta_patched` is always a genuine success by
            // the time it reaches here.
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
            let Some(episode_idx) = media.idx else {
                return Ok(None);
            };
            let Some(meta_ep) = meta_arc
                .videos
                .as_deref()
                .and_then(|v| {
                    match_episode_video(
                        v,
                        season_idx,
                        episode_idx,
                        &media.title,
                        media
                            .released_at
                            .map(|d| d.date()),
                        media
                            .external_ids
                            .tvdb,
                    )
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
            // The matched video can sit at a different number when the addon
            // splits or merges episodes differently from where this episode
            // came from (e.g. TMDB vs IMDb double episodes). Keep our own
            // numbering; only the video id and metadata come from the addon.
            found.idx = media.idx;
            let relations = build_episode_relations(media, meta_ep);
            if !relations.is_empty() {
                found.relations = Some(relations);
            }
            Ok(Some(found))
        }
        _ => Ok(None),
    }
}

/// Finds the addon video for the episode we know as `season`x`episode`.
///
/// Our episode list may come from a different source than the addon's (TMDB
/// vs Cinemeta/IMDb), and the two don't always agree on numbering: TMDB merges
/// some double episodes that IMDb keeps as two parts, so everything after the
/// merge sits one number lower than the addon's video for it. Matching on the
/// number alone pins the wrong video id, and with it the wrong streams.
///
/// So the number is only trusted when nothing contradicts it. In order:
/// 1. the video carrying our TVDB episode id, when we have one;
/// 2. the video at the same number, if its title matches ours;
/// 3. the first video in the season whose title matches ours, ignoring part
///    suffixes like "(1)" (a merged episode maps to its first part);
/// 4. the first video that aired on our air date, but only when the video at
///    our number is clearly a different airing and the season's dates are
///    actually distinct (some addons stamp a whole season with one date);
/// 5. the video at the same number, as before.
fn match_episode_video<'a>(
    videos: &'a [sdks::stremio::Episode],
    season: i64,
    episode: i64,
    title: &str,
    aired: Option<chrono::NaiveDate>,
    tvdb: Option<i64>,
) -> Option<&'a sdks::stremio::Episode> {
    let in_season: Vec<&sdks::stremio::Episode> = videos
        .iter()
        .filter(|v| v.season == Some(season))
        .collect();
    if let Some(v) = tvdb.and_then(|id| {
        in_season
            .iter()
            .copied()
            .find(|v| v.tvdb_id == Some(id))
    }) {
        return Some(v);
    }
    let by_index = in_season
        .iter()
        .copied()
        .find(|v| v.episode == Some(episode));
    let video_date = |v: &sdks::stremio::Episode| {
        v.released
            .map(|d| d.date_naive())
    };

    // A date shared by more than two videos isn't an air date anyone can
    // match on (two is a double episode aired the same night).
    let mut per_date: std::collections::HashMap<chrono::NaiveDate, usize> =
        std::collections::HashMap::new();
    for d in in_season
        .iter()
        .filter_map(|v| video_date(v))
    {
        *per_date
            .entry(d)
            .or_default() += 1;
    }
    let dates_usable = per_date
        .values()
        .all(|&n| n <= 2);
    let near = |v: &sdks::stremio::Episode| match (aired, video_date(v)) {
        (Some(a), Some(d)) => {
            (a - d)
                .num_days()
                .abs()
                <= 1
        }
        _ => false,
    };
    let first = |matches: Vec<&'a sdks::stremio::Episode>| {
        matches
            .into_iter()
            .min_by_key(|v| v.episode)
    };

    let key = episode_title_key(title);
    if !key.is_empty() {
        let title_of = |v: &sdks::stremio::Episode| {
            v.get_name()
                .map(|n| episode_title_key(&n))
                .unwrap_or_default()
        };
        if let Some(v) = by_index.filter(|v| title_of(v) == key) {
            return Some(v);
        }
        // A title match elsewhere still has to agree with the air date when
        // there is one to check against, so repeated placeholder titles
        // ("TBA") can't pull an episode onto a different airing.
        let matches: Vec<_> = in_season
            .iter()
            .copied()
            .filter(|v| title_of(v) == key)
            .filter(|v| {
                !dates_usable || aired.is_none() || video_date(v).is_none() || near(v)
            })
            .collect();
        if let Some(v) = first(matches) {
            return Some(v);
        }
    }

    if dates_usable && !by_index.is_some_and(|v| near(v)) {
        if let Some(a) = aired {
            let matches: Vec<_> = in_season
                .iter()
                .copied()
                .filter(|v| video_date(v) == Some(a))
                .collect();
            if let Some(v) = first(matches) {
                return Some(v);
            }
        }
    }

    by_index
}

/// Normalises an episode title for matching across sources: case, punctuation
/// and parenthesised parts ("(1)", "(a.k.a. ...)") are dropped, as are a
/// trailing "Part N" and articles ("The Manager and the Salesman" vs "Manager
/// and Salesman"), and a possessive "'s" is folded so "Ross's" and "Ross'"
/// compare equal.
fn episode_title_key(title: &str) -> String {
    let mut stripped = String::with_capacity(title.len());
    let mut depth = 0usize;
    for c in title
        .to_lowercase()
        .chars()
    {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            _ if depth == 0 => stripped.push(if c == '\u{2019}' { '\'' } else { c }),
            _ => {}
        }
    }
    let mut words: Vec<String> = stripped
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .map(|w| {
            let w = w
                .strip_suffix("'s")
                .unwrap_or(w);
            w.replace('\'', "")
        })
        .filter(|w| !w.is_empty())
        .collect();
    if words.len() > 2
        && words[words.len() - 2] == "part"
        && words
            .last()
            .is_some_and(|w| {
                w.chars()
                    .all(|c| c.is_ascii_digit())
                    || matches!(
                        w.as_str(),
                        "one" | "two" | "three" | "i" | "ii" | "iii"
                    )
            })
    {
        words.truncate(words.len() - 2);
    }
    let is_article = |w: &String| matches!(w.as_str(), "the" | "a" | "an");
    if words
        .iter()
        .any(|w| !is_article(w))
    {
        words.retain(|w| !is_article(w));
    }
    words.concat()
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

    let mut media: Vec<db::Media> = results
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

/// Rewrite a URL whose host is unreachable outside the addon's own network
/// (e.g. AIOStreams in Docker returning its internal `aiostreams` hostname) to
/// use the stremio addon's manifest origin instead. Applied at descriptor
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
        .map(crate::stream::is_internal_host)
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

async fn stremio_streams(
    svc: &stremio_service::StremioService,
    manifest_url: &StremioManifestUrl,
    media: &db::Media,
    id_prefixes: Option<&[String]>,
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
    let mut streams_opt: Option<Vec<sdks::stremio::Stream>> = None;
    for id in ids_to_try {
        match svc
            .get_streams(media_type.clone(), id)
            .await
        {
            Ok(s) => {
                streams_opt = Some(s);
                break;
            }
            Err(e) if is_404(&e) => {
                last_err = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    let streams = match streams_opt {
        Some(s) => s,
        None => return Err(last_err.unwrap_or_else(|| anyhow!("no streams found"))),
    };

    Ok(streams
        .into_iter()
        .filter(|s| s.is_valid())
        .filter_map(|s| {
            let sd = s
                .stream_data
                .as_ref();
            let metadata = stremio_stream_metadata(&s);
            let descriptor = if s.is_torrent() {
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
                        .info_hash()?
                        .to_ascii_lowercase(),
                    file_hint: metadata
                        .filename
                        .clone(),
                    file_idx: metadata.file_idx,
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
                stream_addon: sd.and_then(|d| {
                    d.addon
                        .clone()
                }),
                service_id: sd
                    .and_then(|d| {
                        d.service
                            .as_ref()
                    })
                    .and_then(|s| {
                        s.id.as_deref()
                    })
                    .map(|s| s.to_lowercase()),
                service_cached: s.cached_status(),
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
    fn string_resources_inherit_manifest_types_and_id_prefixes() {
        let manifest: sdks::stremio::Manifest =
            serde_json::from_value(serde_json::json!({
                "id": "test", "name": "Test", "version": "1.0.0",
                "resources": [
                    "meta",
                    { "name": "stream", "types": ["movie"] }
                ],
                "types": ["movie", "series"],
                "idPrefixes": ["tt", "tmdb:"]
            }))
            .unwrap();

        let (resources, _) = parse_manifest_info(&manifest);
        let meta = resources
            .iter()
            .find(|r| r.name == ResourceType::Meta)
            .unwrap();
        assert_eq!(
            meta.id_prefixes,
            Some(vec!["tt".to_string(), "tmdb:".to_string()])
        );
        assert_eq!(meta.types, vec!["movie".to_string(), "series".to_string()]);

        let stream = resources
            .iter()
            .find(|r| r.name == ResourceType::Stream)
            .unwrap();
        assert_eq!(stream.id_prefixes, None);
    }

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

    #[test]
    fn rewrite_aio_url_remaps_internal_host_to_manifest_origin() {
        let manifest_url =
            StremioManifestUrl::try_new("https://addon.example.com:8443/manifest.json")
                .unwrap();
        let rewritten =
            rewrite_aio_url("http://aiostreams:3000/stream/foo.mkv", &manifest_url);
        assert_eq!(rewritten, "https://addon.example.com:8443/stream/foo.mkv");
    }

    #[test]
    fn rewrite_aio_url_remaps_private_ip_host() {
        let manifest_url =
            StremioManifestUrl::try_new("https://addon.example.com/manifest.json")
                .unwrap();
        let rewritten =
            rewrite_aio_url("http://192.168.1.50:11470/stream/foo.mkv", &manifest_url);
        assert_eq!(rewritten, "https://addon.example.com/stream/foo.mkv");
    }

    #[test]
    fn rewrite_aio_url_leaves_public_host_untouched() {
        let manifest_url =
            StremioManifestUrl::try_new("https://addon.example.com/manifest.json")
                .unwrap();
        let url = "https://cdn.example.net/stream/foo.mkv";
        assert_eq!(rewrite_aio_url(url, &manifest_url), url);
    }

    #[test]
    fn stremio_catalogs_can_be_provided_by_user_scoped_addons() {
        // Imported catalogs and their items are library-wide, so a user-scoped
        // Stremio addon may provide them like any other addon.
        assert!(
            StremioPreset
                .metadata()
                .supported_resources_user
                .contains(&ResourceType::Catalog)
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
                .json_body(serde_json::json!({"streams": [{
                    "url": "https://example.com/1.mp4",
                    "streamData": {"service": {"id": "torbox", "cached": true}}
                }]}));
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
        media.grandparent = Some(Arc::new(grandparent));

        let streams = stremio_streams(&svc, &manifest_url, &media, None)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        assert_eq!(
            streams[0]
                .service_id
                .as_deref(),
            Some("torbox")
        );
        assert_eq!(streams[0].service_cached, Some(true));
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
        media.grandparent = Some(Arc::new(grandparent));

        let streams = stremio_streams(&svc, &manifest_url, &media, None)
            .await
            .unwrap();

        assert_eq!(streams.len(), 1);
        reconstructed.assert();
    }

    fn season_videos(
        season: i64,
        rows: &[(i64, &str, &str)],
    ) -> Vec<sdks::stremio::Episode> {
        rows.iter()
            .map(|(ep, name, released)| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("tt0000000:{season}:{ep}"),
                    "name": name,
                    "season": season,
                    "episode": ep,
                    "released": format!("{released}T00:00:00.000Z"),
                }))
                .unwrap()
            })
            .collect()
    }

    fn date(s: &str) -> Option<chrono::NaiveDate> {
        chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d").ok()
    }

    /// Cinemeta's (IMDb's) season 6 of Friends: 25 videos, with "The One That
    /// Could Have Been" and "The One with the Proposal" in two parts each.
    const FRIENDS_S6_VIDEOS: &[(i64, &str, &str)] = &[
        (1, "The One After Vegas", "1999-09-23"),
        (2, "The One Where Ross Hugs Rachel", "1999-09-30"),
        (3, "The One With Ross' Denial", "1999-10-07"),
        (4, "The One Where Joey Loses His Insurance", "1999-10-14"),
        (5, "The One With Joey's Porsche", "1999-10-21"),
        (6, "The One On The Last Night", "1999-11-04"),
        (7, "The One Where Phoebe Runs", "1999-11-11"),
        (8, "The One With Ross' Teeth", "1999-11-18"),
        (9, "The One Where Ross Got High", "1999-11-25"),
        (
            10,
            "The One With The Routine (a.k.a. The One With The Rockin' New Year)",
            "1999-12-16",
        ),
        (11, "The One With The Apothecary Table", "2000-01-06"),
        (12, "The One With The Joke", "2000-01-13"),
        (13, "The One With Rachel's Sister (1)", "2000-02-03"),
        (14, "The One Where Chandler Can't Cry (2)", "2000-02-10"),
        (15, "The One That Could Have Been (1)", "2000-02-17"),
        (16, "The One That Could Have Been (2)", "2000-02-17"),
        (
            17,
            "The One With Unagi (a.k.a. The One With The Mix Tape)",
            "2000-02-24",
        ),
        (18, "The One Where Ross Dates A Student", "2000-03-09"),
        (19, "The One With Joey's Fridge", "2000-03-23"),
        (20, "The One With Mac And C.H.E.E.S.E.", "2000-04-13"),
        (21, "The One Where Ross Meets Elizabeth's Dad", "2000-04-27"),
        (22, "The One Where Paul's The Man", "2000-05-04"),
        (23, "The One With The Ring", "2000-05-11"),
        (24, "The One With The Proposal (1)", "2000-05-18"),
        (25, "The One With The Proposal (2)", "2000-05-18"),
    ];

    /// TMDB's season 6 of Friends: 23 episodes, both double episodes merged.
    const FRIENDS_S6_TMDB: &[(i64, &str, &str)] = &[
        (1, "The One After Vegas", "1999-09-23"),
        (2, "The One Where Ross Hugs Rachel", "1999-09-30"),
        (3, "The One with Ross's Denial", "1999-10-07"),
        (4, "The One Where Joey Loses His Insurance", "1999-10-14"),
        (5, "The One with Joey's Porsche", "1999-10-21"),
        (6, "The One on the Last Night", "1999-11-04"),
        (7, "The One Where Phoebe Runs", "1999-11-11"),
        (8, "The One with Ross's Teeth", "1999-11-18"),
        (9, "The One Where Ross Got High", "1999-11-25"),
        (10, "The One with the Routine", "1999-12-16"),
        (11, "The One with the Apothecary Table", "2000-01-06"),
        (12, "The One with the Joke", "2000-01-13"),
        (13, "The One with Rachel's Sister (1)", "2000-02-03"),
        (14, "The One Where Chandler Can't Cry (2)", "2000-02-10"),
        (15, "The One That Could Have Been", "2000-02-17"),
        (16, "The One with Unagi", "2000-02-24"),
        (17, "The One Where Ross Dates a Student", "2000-03-09"),
        (18, "The One with Joey's Fridge", "2000-03-23"),
        (19, "The One with Mac and C.H.E.E.S.E.", "2000-04-13"),
        (20, "The One Where Ross Meets Elizabeth's Dad", "2000-04-27"),
        (21, "The One Where Paul's the Man", "2000-05-04"),
        (22, "The One with the Ring", "2000-05-11"),
        (23, "The One with the Proposal", "2000-05-18"),
    ];

    /// Expected addon episode for each TMDB episode 1..=23.
    const FRIENDS_S6_EXPECTED: [i64; 23] = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 17, 18, 19, 20, 21, 22, 23,
        24,
    ];

    #[test]
    fn match_episode_video_follows_merged_double_episodes() {
        let videos = season_videos(6, FRIENDS_S6_VIDEOS);
        for ((idx, title, aired), expected) in FRIENDS_S6_TMDB
            .iter()
            .zip(FRIENDS_S6_EXPECTED)
        {
            let v = match_episode_video(&videos, 6, *idx, title, date(aired), None)
                .unwrap();
            assert_eq!(v.episode, Some(expected), "TMDB S06E{idx:02} {title}");
        }
    }

    #[test]
    fn match_episode_video_falls_back_to_air_date_when_titles_differ() {
        // e.g. titles in another language than the addon's
        let videos = season_videos(6, FRIENDS_S6_VIDEOS);
        for ((idx, _, aired), expected) in FRIENDS_S6_TMDB
            .iter()
            .zip(FRIENDS_S6_EXPECTED)
        {
            let v = match_episode_video(
                &videos,
                6,
                *idx,
                &format!("Episodio {idx}"),
                date(aired),
                None,
            )
            .unwrap();
            assert_eq!(v.episode, Some(expected), "TMDB S06E{idx:02}");
        }
    }

    /// Cinemeta's (IMDb's) season 6 of The Office: 26 videos, "Niagara" and
    /// "The Delivery" in two parts, every one dated 2001-01-31.
    const OFFICE_S6_VIDEOS: &[(i64, &str, i64)] = &[
        (1, "Gossip", 796411),
        (2, "The Meeting", 1087261),
        (3, "The Promotion", 1112611),
        (4, "Niagara (1)", 1112621),
        (5, "Niagara (2)", 4077499),
        (6, "Mafia", 1148281),
        (7, "The Lover", 1160281),
        (8, "Koi Pond", 1181921),
        (9, "Double Date", 1190891),
        (10, "Murder", 1271791),
        (11, "Shareholder Meeting", 1271801),
        (12, "Scott's Tots", 1319731),
        (13, "Secret Santa", 1319741),
        (14, "The Banker", 1511401),
        (15, "Sabre", 1511421),
        (16, "Manager and Salesman", 1602631),
        (17, "The Delivery (1)", 1832421),
        (18, "The Delivery (2)", 1511411),
        (19, "St. Patrick's Day", 1775891),
        (20, "New Leads", 1692591),
        (21, "Happy Hour", 1836991),
        (22, "Secretary's Day", 1985551),
        (23, "Body Language", 2046171),
        (24, "The Cover-Up", 2083471),
        (25, "The Chump", 2121381),
        (26, "Whistleblower", 2161041),
    ];

    /// TMDB's season 6 of The Office: 24 episodes.
    const OFFICE_S6_TMDB: &[(i64, &str, &str)] = &[
        (1, "Gossip", "2009-09-17"),
        (2, "The Meeting", "2009-09-24"),
        (3, "The Promotion", "2009-10-01"),
        (4, "Niagara", "2009-10-08"),
        (5, "Mafia", "2009-10-15"),
        (6, "The Lover", "2009-10-22"),
        (7, "Koi Pond", "2009-10-29"),
        (8, "Double Date", "2009-11-05"),
        (9, "Murder", "2009-11-12"),
        (10, "Shareholder Meeting", "2009-11-19"),
        (11, "Scott's Tots", "2009-12-03"),
        (12, "Secret Santa", "2009-12-10"),
        (13, "The Banker", "2010-01-21"),
        (14, "Sabre", "2010-02-04"),
        (15, "The Manager and the Salesman", "2010-02-11"),
        (16, "The Delivery", "2010-03-04"),
        (17, "St. Patrick's Day", "2010-03-11"),
        (18, "New Leads", "2010-03-18"),
        (19, "Happy Hour", "2010-03-25"),
        (20, "Secretary's Day", "2010-04-22"),
        (21, "Body Language", "2010-04-29"),
        (22, "The Cover-Up", "2010-05-06"),
        (23, "The Chump", "2010-05-13"),
        (24, "Whistleblower", "2010-05-20"),
    ];

    const OFFICE_S6_EXPECTED: [i64; 24] = [
        1, 2, 3, 4, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23, 24,
        25, 26,
    ];

    fn office_s6_videos() -> Vec<sdks::stremio::Episode> {
        OFFICE_S6_VIDEOS
            .iter()
            .map(|(ep, name, tvdb)| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("tt0386676:6:{ep}"),
                    "name": name,
                    "season": 6,
                    "episode": ep,
                    "tvdb_id": tvdb,
                    "released": "2001-01-31T00:00:00.000Z",
                }))
                .unwrap()
            })
            .collect()
    }

    #[test]
    fn match_episode_video_follows_split_hour_long_episodes_by_title() {
        let videos = office_s6_videos();
        for ((idx, title, aired), expected) in OFFICE_S6_TMDB
            .iter()
            .zip(OFFICE_S6_EXPECTED)
        {
            let v = match_episode_video(&videos, 6, *idx, title, date(aired), None)
                .unwrap();
            assert_eq!(v.episode, Some(expected), "TMDB S06E{idx:02} {title}");
        }
        // Every video shares one date, so with no title to go on the number stands.
        let v =
            match_episode_video(&videos, 6, 5, "Episodio 5", date("2009-10-15"), None)
                .unwrap();
        assert_eq!(v.episode, Some(5));
    }

    #[test]
    fn match_episode_video_prefers_the_tvdb_episode_id() {
        let videos = office_s6_videos();
        // TMDB's TVDB ids for S06E04, E05, E16 and E24, with titles that match nothing.
        for (idx, tvdb, expected) in [
            (4, 1112621, 4),
            (5, 1148281, 6),
            (16, 1832421, 17),
            (24, 2161041, 26),
        ] {
            let v = match_episode_video(&videos, 6, idx, "Episodio", None, Some(tvdb))
                .unwrap();
            assert_eq!(v.episode, Some(expected), "TMDB S06E{idx:02}");
        }
    }

    #[test]
    fn match_episode_video_keeps_the_number_without_clear_evidence() {
        let videos = season_videos(
            1,
            &[
                (1, "Pilot", "2026-09-07"),
                (2, "TBA", "2026-09-08"),
                (3, "TBA", "2026-09-09"),
                (4, "TBA", "2026-09-10"),
            ],
        );
        // A repeated placeholder title doesn't pull an episode to another airing.
        let v = match_episode_video(&videos, 1, 4, "TBA", date("2026-09-10"), None)
            .unwrap();
        assert_eq!(v.episode, Some(4));
        // A one-day air date skew on a daily show isn't a different episode.
        let v =
            match_episode_video(&videos, 1, 3, "Episodio 3", date("2026-09-08"), None)
                .unwrap();
        assert_eq!(v.episode, Some(3));
        // Missing date and title: the number as before.
        let v = match_episode_video(&videos, 1, 2, "", None, None).unwrap();
        assert_eq!(v.episode, Some(2));
        assert!(match_episode_video(&videos, 2, 1, "Pilot", None, None).is_none());
    }

    #[tokio::test]
    async fn stremio_meta_fetch_pins_the_matching_video_for_a_tmdb_episode() {
        let (_server, guard) = crate::integration_test::new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = httpmock::MockServer::start();
        let whistleblower = addon.mock(|when, then| {
            when.path("/stream/series/tt0386676:6:26.json");
            then.status(200)
                .json_body(serde_json::json!({"streams": [{"url": "https://example.com/6x26.mkv"}]}));
        });
        let svc = stremio_service::StremioService::from_url(&addon.base_url()).unwrap();
        let manifest_url = StremioManifestUrl::try_new(addon.base_url()).unwrap();

        let meta = sdks::stremio::Meta {
            videos: Some(office_s6_videos()),
            ..serde_json::from_value(serde_json::json!({
                "id": "tt0386676",
                "imdb_id": "tt0386676",
                "type": "series",
                "name": "The Office",
            }))
            .unwrap()
        };
        let cache = std::sync::Mutex::new(std::collections::HashMap::from([(
            "tt0386676".to_string(),
            Arc::new(meta),
        )]));
        let failed = std::sync::Mutex::new(std::collections::HashSet::new());

        let series = db::Media {
            id: Uuid::new_v4(),
            kind: db::MediaKind::Series,
            title: "The Office".into(),
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0386676".to_string()).ok(),
                tmdb: Some(2316),
                ..Default::default()
            },
            ..Default::default()
        };
        // TMDB's S06E24, which IMDb numbers S06E26.
        let mut episode = db::Media {
            id: Uuid::new_v4(),
            kind: db::MediaKind::Episode,
            title: "Whistleblower".into(),
            idx: Some(24),
            parent_idx: Some(6),
            parent_id: Some(Uuid::new_v4()),
            grandparent_id: Some(series.id),
            released_at: date("2010-05-20").and_then(|d| d.and_hms_opt(0, 0, 0)),
            ..Default::default()
        };
        episode.grandparent = Some(Arc::new(series));

        let found = stremio_meta_fetch(&svc, &episode, ctx, &cache, &failed)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            found
                .external_ids
                .custom_stremio_id
                .as_deref(),
            Some("tt0386676:6:26")
        );
        assert_eq!(found.idx, Some(24));

        episode
            .external_ids
            .merge(&found.external_ids, false);
        let streams = stremio_streams(&svc, &manifest_url, &episode, None)
            .await
            .unwrap();
        assert_eq!(streams.len(), 1);
        whistleblower.assert();
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
}
