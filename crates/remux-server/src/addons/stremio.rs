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
    services::stremio as stremio_service,
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
        let client = super::make_http_client();
        let addon = Arc::new(StremioAddon {
            manifest_url,
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
                .unwrap_or(remux_sdks::stremio::MediaType::Unknown(s.clone()))
        })
        .collect();
    (resources, types)
}

#[nutype(
    sanitize(trim, with = |s: String| {
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
                        remux_sdks::stremio::MediaType::Unknown(
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
                let meta_id = root
                    .external_ids
                    .stremio_lookup_id()
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
                // Rebuild series external_ids from what the season carries.
                let series_external_ids = db::ExternalIds {
                    imdb: root
                        .external_ids
                        .series_imdb
                        .clone(),
                    tmdb: root
                        .external_ids
                        .series_tmdb,
                    custom_stremio_id: root
                        .external_ids
                        .series_custom_stremio_id
                        .clone(),
                    ..Default::default()
                };
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
                    if let Some(ep_num) = ep.idx {
                        if let Some(s_num) = ep.parent_idx {
                            ep.title = format!("S{}E{} - {}", s_num, ep_num, ep.title);
                        } else {
                            ep.title = format!("E{} - {}", ep_num, ep.title);
                        }
                    }
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
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let svc = self.service()?;
        stremio_streams(&svc, &self.manifest_url, media).await
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
                    crate::addons::tmdb::resolve_imdb_from_ids(&ids, is_tv, client)
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
    let meta_id: String = media
        .external_ids
        .stremio_lookup_id()
        .ok_or_else(|| anyhow!("no resolvable meta id for {}", media.id))?;

    if let Some(cached) = cache
        .lock()
        .unwrap()
        .get(&meta_id)
        .cloned()
    {
        return Ok(cached);
    }

    let is_custom = media
        .external_ids
        .imdb
        .is_none()
        && media
            .external_ids
            .series_imdb
            .is_none();
    let media_type = sdks::stremio::MediaType::from(&media.kind);
    let meta = if let Some(stored) = ctx
        .store
        .get::<sdks::stremio::Meta>(
            media
                .id
                .to_string(),
        ) {
        stored
    } else {
        match svc
            .get_meta(media_type.clone(), meta_id.clone())
            .await
        {
            Ok(m) => m,
            Err(e) if is_404(&e) && !is_custom => {
                let tmdb_id = media
                    .external_ids
                    .tmdb
                    .or(media
                        .external_ids
                        .series_tmdb);
                if let Some(tid) = tmdb_id {
                    svc.get_meta(media_type, format!("tmdb:{}", tid))
                        .await?
                } else {
                    return Err(e);
                }
            }
            Err(e) => return Err(e),
        }
    };

    let arc = Arc::new(meta);
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
        .external_ids
        .series_imdb
        .clone()
        .or(media
            .external_ids
            .imdb
            .clone());
    let is_custom = imdb_id.is_none();

    let meta_arc = fetch_and_cache_meta(svc, media, medias_cache, ctx).await?;

    // Patch imdb_id into a mutable clone for root-level conversion and relations.
    let mut meta_patched = (*meta_arc).clone();
    if meta_patched
        .imdb_id
        .is_none()
        && !is_custom
    {
        meta_patched.imdb_id = db::ExternalIds::from_stremio_id(&meta_patched.id)
            .imdb
            .map(Into::into)
            .or_else(|| imdb_id.map(Into::into));
    }

    match media.kind {
        db::MediaKind::Movie | db::MediaKind::Series => {
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
            let series_external_ids = db::ExternalIds {
                imdb: media
                    .external_ids
                    .series_imdb
                    .clone(),
                tmdb: media
                    .external_ids
                    .series_tmdb,
                custom_stremio_id: media
                    .external_ids
                    .series_custom_stremio_id
                    .clone(),
                ..Default::default()
            };
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
            let series_external_ids = db::ExternalIds {
                imdb: media
                    .external_ids
                    .series_imdb
                    .clone(),
                tmdb: media
                    .external_ids
                    .series_tmdb,
                custom_stremio_id: media
                    .external_ids
                    .series_custom_stremio_id
                    .clone(),
                ..Default::default()
            };
            let episodes = db::stremio_meta_season_episodes(
                &meta_arc,
                series_id,
                season_id,
                season_idx,
                &series_external_ids,
            )?;
            let mut found = episodes
                .into_iter()
                .find(|e| e.idx == media.idx);
            if let Some(ref mut ep) = found {
                if let Some(meta_ep) = meta_patched
                    .videos
                    .as_ref()
                    .and_then(|v| {
                        v.iter()
                            .find(|e| {
                                e.episode == media.idx && e.season == media.parent_idx
                            })
                    })
                {
                    let relations = build_episode_relations(media, meta_ep);
                    if !relations.is_empty() {
                        ep.relations = Some(relations);
                    }
                }
            }
            Ok(found)
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
                .external_ids
                .series_imdb
                .as_deref()
                .ok_or_else(|| anyhow!("no series_imdb"))?,
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

async fn stremio_streams(
    svc: &stremio_service::StremioService,
    manifest_url: &StremioManifestUrl,
    media: &db::Media,
) -> Result<Vec<crate::stream::StreamInfo>> {
    let (media_type, id, tmdb_fallback_id) = match media.kind {
        db::MediaKind::Episode => {
            let series_id: String = media
                .external_ids
                .series_imdb
                .as_deref()
                .map(|s| s.to_string())
                .or_else(|| media.external_ids.series_custom_stremio_id.clone())
                .ok_or_else(|| {
                    anyhow!("episode has no series_imdb or series_custom_stremio_id for stream lookup")
                })?;
            let season = media
                .parent_idx
                .unwrap_or(1);
            let episode = media
                .idx
                .unwrap_or(1);
            let tmdb_fb = media
                .external_ids
                .series_tmdb
                .map(|tid| format!("tmdb:{}:{}:{}", tid, season, episode));
            (
                sdks::stremio::MediaType::Series,
                format!("{}:{}:{}", series_id, season, episode),
                tmdb_fb,
            )
        }
        db::MediaKind::Track => {
            let id = media
                .external_ids
                .deezer_track
                .map(|n| format!("deezer:{n}"))
                .ok_or_else(|| {
                    anyhow!("track has no deezer ID for Stremio stream lookup")
                })?;
            (sdks::stremio::MediaType::Track, id, None)
        }
        db::MediaKind::Album => {
            let id = media
                .external_ids
                .deezer_album
                .map(|n| format!("deezer:{n}"))
                .ok_or_else(|| {
                    anyhow!("album has no deezer ID for Stremio stream lookup")
                })?;
            (sdks::stremio::MediaType::Album, id, None)
        }
        db::MediaKind::Artist => {
            let id = media
                .external_ids
                .deezer_artist
                .map(|n| format!("deezer:{n}"))
                .ok_or_else(|| {
                    anyhow!("artist has no deezer ID for Stremio stream lookup")
                })?;
            (sdks::stremio::MediaType::Artist, id, None)
        }
        _ => {
            let id = media
                .external_ids
                .imdb
                .as_deref()
                .map(|s| s.to_string())
                .ok_or_else(|| {
                    anyhow!("media has no identifiable ID for Stremio stream lookup")
                })?;
            let tmdb_fb = media
                .external_ids
                .tmdb
                .map(|tid| format!("tmdb:{}", tid));
            (sdks::stremio::MediaType::from(&media.kind), id, tmdb_fb)
        }
    };

    let streams = match svc
        .get_streams(media_type.clone(), id)
        .await
    {
        Ok(s) => s,
        Err(e) if is_404(&e) => {
            if let Some(fb_id) = tmdb_fallback_id {
                svc.get_streams(media_type, fb_id)
                    .await?
            } else {
                return Err(e);
            }
        }
        Err(e) => return Err(e),
    };

    Ok(streams
        .into_iter()
        .filter(|s| s.is_valid())
        .filter_map(|s| {
            let descriptor = if s.is_torrent() {
                crate::stream::StreamDescriptor::Torrent {
                    info_hash: s
                        .info_hash
                        .clone()?
                        .to_ascii_lowercase(),
                    file_hint: s
                        .filename
                        .clone(),
                    file_idx: s
                        .file_idx
                        .map(|i| i as usize),
                    trackers: s
                        .sources
                        .as_deref()
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|src| src.strip_prefix("tracker:"))
                        .map(String::from)
                        .collect(),
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
            Some(crate::stream::StreamInfo {
                descriptor,
                name: Some(label),
                description: s
                    .description
                    .clone(),
                filename: s
                    .behavior_hints
                    .as_ref()
                    .and_then(|bh| {
                        bh.filename
                            .clone()
                    })
                    .or_else(|| {
                        s.filename
                            .clone()
                    }),
                seeders: s.seeders,
                size: s.size,
                duration: s.duration,
                subtitles: s
                    .subtitles
                    .clone(),
                probe_data: None,
                ..Default::default()
            })
        })
        .collect())
}
