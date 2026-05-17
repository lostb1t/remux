use anyhow::{Result, anyhow};
use async_trait::async_trait;
use chrono::Utc;
use futures::{Stream, StreamExt};
use nutype::nutype;

use serde::{Deserialize, Deserializer};
use sqlx::SqlitePool;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, warn};
use uuid::Uuid;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, CatalogInfo, MediaKind, MusicSearchResult, ResourceType,
    addon,
};
use crate::sdks::CachedEndpoint;
use crate::services::stremio as stremio_service;
use crate::{AppContext, common, db, sdks};

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
                ResourceType::Catalog,
                ResourceType::Meta,
                ResourceType::Search,
                ResourceType::Subtitles,
                ResourceType::Stream,
            ],
            supported_types: vec![MediaKind::Movie, MediaKind::Series],
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
    ) -> Result<Arc<dyn AddonKind>> {
        let raw_url = cfg
            .get("manifest_url")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow!("Stremio addon missing manifest_url in config"))?
            .to_string();
        let manifest_url = StremioManifestUrl::try_new(raw_url)
            .map_err(|e| anyhow!("Invalid manifest_url: {e}"))?;
        let client = reqwest::Client::builder()
            .user_agent("remux-server/1.0")
            .build()?;
        Ok(Arc::new(StremioAddon {
            manifest_url,
            client,
        }))
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(StremioPreset))
}

fn parse_manifest_info(
    manifest: &remux_sdks::stremio::Manifest,
) -> (Vec<ResourceType>, Vec<remux_sdks::stremio::MediaType>) {
    let mut resources = Vec::new();
    for res in &manifest.resources {
        match res.resource_type() {
            ResourceType::Catalog => {
                if !resources.contains(&ResourceType::Catalog) {
                    resources.push(ResourceType::Catalog);
                }
            }
            ResourceType::Meta => {
                if !resources.contains(&ResourceType::Meta) {
                    resources.push(ResourceType::Meta);
                }
            }
            ResourceType::Subtitles => {
                if !resources.contains(&ResourceType::Subtitles) {
                    resources.push(ResourceType::Subtitles);
                }
            }
            ResourceType::Stream => {
                if !resources.contains(&ResourceType::Stream) {
                    resources.push(ResourceType::Stream);
                }
            }
            _ => {}
        }
    }
    if manifest
        .catalogs
        .iter()
        .any(|c| c.extra.iter().any(|e| e.name == "search"))
    {
        if !resources.contains(&ResourceType::Search) {
            resources.push(ResourceType::Search);
        }
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
    ) -> (Vec<ResourceType>, Vec<remux_sdks::stremio::MediaType>) {
        let Ok(svc) = self.service() else {
            return (vec![], vec![]);
        };
        let Ok(manifest) = svc.get_manifest().await else {
            return (vec![], vec![]);
        };
        parse_manifest_info(&manifest)
    }

    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        let svc = self.service()?;
        let manifest = svc.get_manifest().await?;
        Ok(manifest
            .catalogs
            .into_iter()
            .filter(|c| !c.id.contains("search"))
            .map(|c| {
                CatalogInfo::new(
                    format!("{}:{}", c.kind.to_lowercase(), c.id),
                    format!("{} — {}", manifest.name.trim(), c.name.trim()),
                )
            })
            .collect())
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        let svc = self.service()?;
        let manifest = svc.get_manifest().await?;

        let cat = manifest
            .catalogs
            .into_iter()
            .find(|c| format!("{}:{}", c.kind.to_lowercase(), c.id) == local_id)
            .ok_or_else(|| {
                anyhow!(
                    "catalog '{}' not found in Stremio manifest {}",
                    local_id,
                    self.manifest_url
                )
            })?;

        let stream = svc.get_catalog_stream(&cat).await?;
        let tmdb_client = crate::common::tmdb_client(&ctx.db).await;

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

    async fn meta_supports(&self, media: &db::Media) -> bool {
        stremio_type_for_kind(&media.kind).is_some()
    }

    async fn meta_fetch(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let svc = self.service()?;
        stremio_meta_fetch(&svc, media, ctx).await
    }

    fn tree_supports(&self, root: &db::Media) -> bool {
        root.kind == db::MediaKind::Series
    }

    async fn tree_sync_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if root.kind != db::MediaKind::Series {
            return Ok(None);
        }
        let svc = self.service()?;
        let children = stremio_sync_children(&svc, root, ctx).await?;
        if children.is_empty() {
            Ok(None)
        } else {
            Ok(Some(children))
        }
    }

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

    async fn search_persist(
        &self,
        id: Uuid,
        ctx: &AppContext,
    ) -> Result<Option<db::Media>> {
        let svc = match self.service() {
            Ok(a) => a,
            Err(_) => return Ok(None),
        };
        stremio_persist(&svc, id, ctx).await
    }

    fn subtitle_supports(&self, media: &db::Media) -> bool {
        matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Episode)
    }

    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        _db: &SqlitePool,
    ) -> Result<Vec<sdks::stremio::Subtitle>> {
        let svc = self.service()?;
        stremio_subtitles(&svc, media).await
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
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
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Catalog helpers
// ---------------------------------------------------------------------------

fn needs_release_dates(meta: &sdks::stremio::Meta) -> bool {
    matches!(meta.media_type, sdks::stremio::MediaType::Movie)
        && meta
            .app_extras
            .as_ref()
            .and_then(|e| e.release_dates.as_ref())
            .is_none()
}

fn inject_tmdb_release_dates(
    meta: &mut sdks::stremio::Meta,
    tmdb_rd: sdks::tmdb::MovieReleaseDates,
) {
    let aio_rd = sdks::stremio::ReleaseDates {
        results: tmdb_rd
            .results
            .into_iter()
            .map(|c| sdks::stremio::ReleaseDateCountry {
                iso_3166_1: c.iso_3166_1,
                release_dates: c
                    .release_dates
                    .into_iter()
                    .filter_map(|rd| {
                        rd.release_date.map(|date| sdks::stremio::ReleaseDateEntry {
                            release_date: date,
                            release_type: rd.release_type,
                        })
                    })
                    .collect(),
            })
            .collect(),
    };
    meta.app_extras
        .get_or_insert_with(Default::default)
        .release_dates = Some(aio_rd);
}

pub(crate) async fn resolve_imdb_id<A: sdks::Auth + Clone>(
    meta: &mut sdks::stremio::Meta,
    svc: Option<&stremio_service::StremioService>,
    tmdb_client: Option<&sdks::RestClient<A>>,
) -> bool {
    if meta.imdb_id.is_none() {
        if let Some(imdb) = db::ExternalIds::from_stremio_id(&meta.id).imdb {
            meta.imdb_id = Some(imdb);
        }
    }

    if meta.imdb_id.is_none() {
        if let Some(svc) = svc {
            match meta.resolve(&svc.client).await {
                Ok(()) => {}
                Err(e) => warn!(id = %meta.id, error = %e, "AIO resolve failed"),
            }
        }
    }

    if meta.imdb_id.is_none() {
        let mut ids = db::ExternalIds::from_stremio_id(&meta.id);
        if ids.tmdb.is_none() {
            ids.tmdb = meta.moviedb_id.map(|n| n as i64);
        }
        if let Some(client) = tmdb_client {
            if !ids.is_empty() {
                let is_tv = meta.media_type == sdks::stremio::MediaType::Series;
                meta.imdb_id =
                    crate::addons::tmdb::resolve_imdb_from_ids(&ids, is_tv, client)
                        .await;
            }
        }
    }

    if meta.imdb_id.is_none() {
        return false;
    }

    if needs_release_dates(meta) {
        if let Some(client) = tmdb_client {
            let tmdb_id = db::ExternalIds::from_stremio_id(&meta.id)
                .tmdb
                .or_else(|| meta.moviedb_id.map(|n| n as i64));

            let tmdb_id = if tmdb_id.is_some() {
                tmdb_id
            } else if let Some(ref imdb_id) = meta.imdb_id {
                client
                    .execute(
                        sdks::tmdb::FindByIdEndpoint {
                            external_id: imdb_id.clone(),
                            external_source: "imdb_id".to_string(),
                        }
                        .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                    .ok()
                    .and_then(|r| r.movie_results.into_iter().next())
                    .map(|m| m.id)
            } else {
                None
            };

            if let Some(tid) = tmdb_id {
                if let Ok(movie) = client
                    .execute(
                        sdks::tmdb::MovieEndpoint::new(tid)
                            .with_cache(Duration::from_secs(3600)),
                    )
                    .await
                {
                    if let Some(rd) = movie.release_dates {
                        inject_tmdb_release_dates(meta, rd);
                    }
                }
            }
        }
    }

    true
}

// ---------------------------------------------------------------------------
// Meta helpers
// ---------------------------------------------------------------------------

async fn stremio_meta_fetch(
    svc: &stremio_service::StremioService,
    media: &db::Media,
    ctx: &AppContext,
) -> Result<Option<db::Media>> {
    let imdb_id = media
        .grandparent_media_id
        .clone()
        .or(media.external_ids.imdb.clone());

    let imdb_id = match imdb_id {
        Some(id) => id,
        None => return Ok(None),
    };

    let mut meta = if let Some(cached_meta) =
        ctx.store.get::<sdks::stremio::Meta>(media.id.to_string())
    {
        cached_meta
    } else {
        svc.get_meta(sdks::stremio::MediaType::from(&media.kind), imdb_id.clone())
            .await?
    };

    if meta.imdb_id.is_none() {
        meta.imdb_id = db::ExternalIds::from_stremio_id(&meta.id)
            .imdb
            .or_else(|| Some(imdb_id.clone()));
    }

    let meta_raw = meta.clone();
    let medias: Vec<db::Media> = db::stremio_meta_to_medias(meta)?;
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

    if let Some(mut found_media) = found {
        let relations =
            if matches!(media.kind, db::MediaKind::Movie | db::MediaKind::Series) {
                build_relations(media, &meta_raw)
            } else if media.kind == db::MediaKind::Episode {
                if let Some(meta_ep) = meta_raw.videos.as_ref().and_then(|v| {
                    v.iter().find(|e| {
                        e.episode == media.idx && e.season == media.parent_idx
                    })
                }) {
                    build_episode_relations(media, meta_ep)
                } else {
                    vec![]
                }
            } else {
                vec![]
            };

        let mut medias = vec![found_media];
        db::Media::enrich_parents(&ctx.db, &mut medias).await;
        found_media = medias.remove(0);

        if !relations.is_empty() {
            found_media.relations = Some(relations);
        }
        Ok(Some(found_media))
    } else {
        Ok(None)
    }
}

async fn stremio_sync_children(
    svc: &stremio_service::StremioService,
    root: &db::Media,
    ctx: &AppContext,
) -> Result<Vec<db::Media>> {
    if root.kind != db::MediaKind::Series {
        return Ok(vec![]);
    }

    let imdb_id = match root.external_ids.imdb.clone() {
        Some(id) => id,
        None => return Ok(vec![]),
    };

    let mut meta = svc
        .get_meta(sdks::stremio::MediaType::from(&root.kind), imdb_id.clone())
        .await?;

    if meta.imdb_id.is_none() {
        meta.imdb_id = db::ExternalIds::from_stremio_id(&meta.id)
            .imdb
            .or_else(|| Some(imdb_id.clone()));
    }

    let meta_clone = meta.clone();
    let medias: Vec<db::Media> = db::stremio_meta_to_medias(meta)?;
    let mut children = medias
        .into_iter()
        .filter_map(|mut x| {
            if x.kind == db::MediaKind::Season {
                x.parent_id = Some(root.id);
                x.grandparent_id = Some(root.id);
                if let Some(url) =
                    x.idx.and_then(|idx| meta_clone.get_season_poster(idx))
                {
                    x.set_image(db::ImageKind::Primary, url);
                }
                x.title = format!("Season {}", x.idx.unwrap_or(1));
                x.refreshed_at = Some(Utc::now().naive_utc());
                Some(x)
            } else if x.kind == db::MediaKind::Episode {
                if let Some(season_idx) = x.parent_idx {
                    let season_media_id = format!("{}:{}", imdb_id, season_idx);
                    x.parent_id = Some(crate::common::get_stable_uuid(season_media_id));
                }
                x.grandparent_id = Some(root.id);
                if let Some(episode_num) = x.idx {
                    if let Some(season_num) = x.parent_idx {
                        x.title =
                            format!("S{}E{} - {}", season_num, episode_num, x.title);
                    } else {
                        x.title = format!("E{} - {}", episode_num, x.title);
                    }
                }
                if super::episode_meta_complete(&x) {
                    x.refreshed_at = Some(Utc::now().naive_utc());
                } else {
                    x.refreshed_at = None;
                }
                Some(x)
            } else {
                None
            }
        })
        .collect();

    db::Media::enrich_parents(&ctx.db, &mut children).await;

    Ok(children)
}

// ---------------------------------------------------------------------------
// Relation builders
// ---------------------------------------------------------------------------

pub(crate) fn build_relations(
    media: &db::Media,
    meta: &sdks::stremio::Meta,
) -> Vec<(db::MediaRelation, db::Media)> {
    let mut relations = Vec::new();

    if let Some(genres) = meta.genre.as_ref().or(meta.genres.as_ref()) {
        for genre_name in genres {
            let genre_id =
                common::get_stable_uuid(format!("genre:{}", genre_name.to_lowercase()));
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
                    media_id: Some(format!("genre:{}", genre_name.to_lowercase())),
                    ..Default::default()
                },
            ));
        }
    }

    let mut rels = build_person_relations(
        media.id,
        meta.director.as_ref(),
        meta.writer.as_ref(),
        None,
        meta.cast.as_ref(),
        None,
        None,
    );

    if let Some(extras) = &meta.app_extras {
        rels.extend(build_person_relations(
            media.id,
            None,
            None,
            extras.cast.as_ref(),
            None,
            extras.directors.as_ref(),
            extras.writers.as_ref(),
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
        ep.directors.as_ref(),
        ep.writers.as_ref(),
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
            .flat_map(|s| s.split(',').map(|n| n.trim().to_string()))
            .filter(|s| !s.is_empty())
            .collect()
    };

    let mut add_members = |members: Option<&Vec<sdks::stremio::CastMember>>,
                           role: db::RelationRole,
                           offset: i64| {
        if let Some(list) = members {
            for (i, member) in list.iter().enumerate() {
                if let Some(name) = &member.name {
                    let name = name.trim().to_string();
                    if name.is_empty() {
                        continue;
                    }
                    let person_id = common::get_stable_uuid(format!(
                        "person:{}",
                        name.to_lowercase()
                    ));
                    let mut person = db::Media {
                        id: person_id,
                        title: name.clone(),
                        kind: db::MediaKind::Person,
                        media_id: Some(format!("person:{}", name.to_lowercase())),
                        ..Default::default()
                    };
                    if let Some(url) = member.photo.clone() {
                        person.set_image(db::ImageKind::Primary, url);
                    }
                    relations.push((
                        db::MediaRelation {
                            left_media_id,
                            right_media_id: person_id,
                            weight: Some(offset + i as i64),
                            role: Some(role.clone()),
                            character: member.character.clone(),
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

    for (i, name) in split_names(cast_names).into_iter().enumerate() {
        let person_id =
            common::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some((i + cast_members.map(|c| c.len()).unwrap_or(0)) as i64),
                role: Some(db::RelationRole::Actor),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
        ));
    }

    for (i, name) in split_names(directors).into_iter().enumerate() {
        let person_id =
            common::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some(
                    (i + director_members.map(|c| c.len()).unwrap_or(0)) as i64,
                ),
                role: Some(db::RelationRole::Director),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
                ..Default::default()
            },
        ));
    }

    for (i, name) in split_names(writers).into_iter().enumerate() {
        let person_id =
            common::get_stable_uuid(format!("person:{}", name.to_lowercase()));
        relations.push((
            db::MediaRelation {
                left_media_id,
                right_media_id: person_id,
                weight: Some((i + writer_members.map(|c| c.len()).unwrap_or(0)) as i64),
                role: Some(db::RelationRole::Writer),
                ..Default::default()
            },
            db::Media {
                id: person_id,
                title: name.clone(),
                kind: db::MediaKind::Person,
                media_id: Some(format!("person:{}", name.to_lowercase())),
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
            ctx.store
                .insert(m.id.to_string(), meta, Duration::from_secs(3600));
            Some(m)
        })
        .collect();

    db::Media::enrich_parents(&ctx.db, &mut media).await;

    Ok(media)
}

async fn stremio_persist(
    svc: &stremio_service::StremioService,
    id: Uuid,
    ctx: &AppContext,
) -> Result<Option<db::Media>> {
    let meta = match ctx.store.get::<sdks::stremio::Meta>(id.to_string()) {
        Some(m) => m,
        None => return Ok(None),
    };

    let mut media: db::Media = svc
        .get_meta(meta.media_type.clone(), meta.id.clone())
        .await?
        .try_into()?;

    media.save(&ctx.db).await.ok();
    ctx.store.delete(id.to_string());

    let saved = db::Media::get_by_filter(
        &ctx.db,
        &db::MediaFilter {
            media_id: media.media_id.clone(),
            ..Default::default()
        },
    )
    .await?
    .records
    .into_iter()
    .next()
    .ok_or_else(|| anyhow::anyhow!("media not found after save"))?;

    Ok(Some(saved))
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
                .grandparent_media_id
                .as_deref()
                .ok_or_else(|| anyhow!("no grandparent_media_id"))?,
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
    let (media_type, id) = match media.kind {
        db::MediaKind::Episode => {
            let series_id = media.grandparent_media_id.as_deref().ok_or_else(|| {
                anyhow!("episode has no grandparent_media_id for stream lookup")
            })?;
            let season = media.parent_idx.unwrap_or(1);
            let episode = media.idx.unwrap_or(1);
            (
                sdks::stremio::MediaType::Series,
                format!("{}:{}:{}", series_id, season, episode),
            )
        }
        _ => {
            let id = media
                .external_ids
                .imdb
                .clone()
                .or_else(|| media.media_id.clone())
                .ok_or_else(|| {
                    anyhow!("media has no identifiable ID for Stremio stream lookup")
                })?;
            (sdks::stremio::MediaType::from(&media.kind), id)
        }
    };

    let streams = svc.get_streams(media_type, id).await?;

    Ok(streams
        .into_iter()
        .filter(|s| s.is_valid())
        .filter_map(|s| {
            let descriptor = if s.is_torrent() {
                crate::stream::StreamDescriptor::Torrent {
                    info_hash: s.info_hash.clone()?.to_ascii_lowercase(),
                    file_hint: s.filename.clone(),
                    file_idx: s.file_idx.map(|i| i as usize),
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
                let url = s.url.clone().or_else(|| s.external_url.clone())?;
                crate::stream::StreamDescriptor::Http {
                    url: rewrite_aio_url(&url, manifest_url),
                    request_headers: s.request_headers.clone(),
                    response_headers: s.response_headers.clone(),
                }
            };
            let label = match (s.name.as_deref(), s.description.as_deref()) {
                (Some(n), Some(d)) if !d.is_empty() => format!("{}\n{}", n, d),
                (Some(n), _) => n.to_string(),
                (None, Some(d)) => d.to_string(),
                _ => "Stream".to_string(),
            };
            Some(crate::stream::StreamInfo {
                descriptor,
                name: Some(label),
                description: s.description.clone(),
                filename: s
                    .behavior_hints
                    .as_ref()
                    .and_then(|bh| bh.filename.clone())
                    .or_else(|| s.filename.clone()),
                seeders: s.seeders,
                size: s.size,
                duration: s.duration,
                subtitles: s.subtitles.clone(),
                probe_data: None,
                ..Default::default()
            })
        })
        .collect())
}
