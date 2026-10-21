use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::TryStreamExt;
use opendal::EntryMode;
use regex::Regex;
use std::{pin::Pin, sync::Arc, time::Duration};
use uuid::Uuid;

use futures::Stream;
use remux_sdks::stremio::MediaType as StremioMediaType;
use tracing::{debug, info, warn};

use super::{
    AddonCapabilities, AddonKind, AddonMetadata, AddonOption, AddonOptionType,
    AddonPreset, AddonPresetRegistration, AddonSelectOption, CatalogAddon, CatalogInfo,
    IndexAddon, MediaKind, ProgressReporter, ResourceType, StreamAddon, SubtitleAddon,
    SubtitleInfo, SubtitleQuery, TreeAddon,
};
use crate::{
    AppContext, addons::Addon, common, db, sdks, sdks::CachedEndpoint,
    services::MediaResolveService,
};

// ---------------------------------------------------------------------------
// Shared option helper
// ---------------------------------------------------------------------------

fn media_kind_option() -> AddonOption {
    AddonOption {
        id: "media_kind".to_string(),
        name: "Content Type".to_string(),
        description: None,
        required: true,
        default: None,
        kind: AddonOptionType::Select {
            options: vec![
                AddonSelectOption {
                    label: "Movies".to_string(),
                    value: "movie".to_string(),
                },
                AddonSelectOption {
                    label: "TV Episodes".to_string(),
                    value: "episode".to_string(),
                },
                AddonSelectOption {
                    label: "Tracks".to_string(),
                    value: "track".to_string(),
                },
            ],
        },
    }
}

fn cfg_paths_local(cfg: &serde_json::Value) -> Result<Vec<String>> {
    if let Some(arr) = cfg["paths"].as_array() {
        let v: Vec<String> = arr
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .collect();
        if !v.is_empty() {
            return Ok(v);
        }
    }
    if let Some(p) = cfg["path"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        return Ok(vec![p.to_string()]);
    }
    anyhow::bail!("opendal-local: at least one path is required")
}

fn cfg_paths_webdav(cfg: &serde_json::Value) -> Vec<String> {
    if let Some(arr) = cfg["paths"].as_array() {
        let v: Vec<String> = arr
            .iter()
            .filter_map(|v| {
                v.as_str()
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
            })
            .collect();
        if !v.is_empty() {
            return v;
        }
    }
    vec!["/".to_string()]
}

// ---------------------------------------------------------------------------
// OpendalLocalPreset
// ---------------------------------------------------------------------------

pub struct OpendalLocalPreset;

impl AddonPreset for OpendalLocalPreset {
    fn id(&self) -> &'static str {
        "opendal-local"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "opendal-local".to_string(),
            display_name: "Local".to_string(),
            description: "Index and stream video or audio files from a local path."
                .to_string(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Stream),
                AddonMetadata::simple_resource(ResourceType::Catalog),
                AddonMetadata::simple_resource(ResourceType::Subtitles),
            ],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Episode,
                MediaKind::Track,
            ],
            supported_resources_user: vec![
                ResourceType::Stream,
                ResourceType::Subtitles,
            ],
            supported_types_user: vec![
                MediaKind::Movie,
                MediaKind::Episode,
                MediaKind::Track,
            ],
            options: vec![
                media_kind_option(),
                AddonOption {
                    id: "paths".to_string(),
                    name: "Paths".to_string(),
                    description: Some("Absolute paths to scan.".to_string()),
                    required: true,
                    default: None,
                    kind: AddonOptionType::StringList,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let media_kind = cfg["media_kind"]
            .as_str()
            .unwrap_or("movie")
            .to_string();
        let paths = cfg_paths_local(cfg)?;
        let first = paths
            .first()
            .cloned()
            .unwrap_or_default();
        let operator =
            opendal::Operator::new(opendal::services::Fs::default().root(&first))?
                .finish();

        let addon = Arc::new(OpendalAddon {
            addon_id,
            operator: Arc::new(operator),
            root: first,
            backend: "local".to_string(),
            media_kind,
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            stream: Some(addon.clone()),
            tree: Some(addon.clone()),
            index: Some(addon.clone()),
            subtitle: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(OpendalLocalPreset))
}

// ---------------------------------------------------------------------------
// OpendalWebdavPreset
// ---------------------------------------------------------------------------

pub struct OpendalWebdavPreset;

impl AddonPreset for OpendalWebdavPreset {
    fn id(&self) -> &'static str {
        "opendal-webdav"
    }

    fn metadata(&self) -> AddonMetadata {
        AddonMetadata {
            id: "opendal-webdav".to_string(),
            display_name: "WebDAV".to_string(),
            description: "Index and stream video or audio files from a WebDAV server."
                .to_string(),
            icon: None,
            supported_resources: vec![
                AddonMetadata::simple_resource(ResourceType::Stream),
                AddonMetadata::simple_resource(ResourceType::Catalog),
                AddonMetadata::simple_resource(ResourceType::Subtitles),
            ],
            supported_types: vec![
                MediaKind::Movie,
                MediaKind::Episode,
                MediaKind::Track,
            ],
            supported_resources_user: vec![
                ResourceType::Stream,
                ResourceType::Subtitles,
            ],
            supported_types_user: vec![
                MediaKind::Movie,
                MediaKind::Episode,
                MediaKind::Track,
            ],
            options: vec![
                media_kind_option(),
                AddonOption {
                    id: "endpoint".to_string(),
                    name: "WebDAV URL".to_string(),
                    description: None,
                    required: true,
                    default: None,
                    kind: AddonOptionType::Url,
                },
                AddonOption {
                    id: "username".to_string(),
                    name: "Username".to_string(),
                    description: None,
                    required: false,
                    default: None,
                    kind: AddonOptionType::String,
                },
                AddonOption {
                    id: "password".to_string(),
                    name: "Password".to_string(),
                    description: None,
                    required: false,
                    default: None,
                    kind: AddonOptionType::Password,
                },
                AddonOption {
                    id: "paths".to_string(),
                    name: "Paths".to_string(),
                    description: Some("Sub-paths to scan (default: /).".to_string()),
                    required: false,
                    default: None,
                    kind: AddonOptionType::StringList,
                },
                AddonOption {
                    id: "incremental_scan".to_string(),
                    name: "Incremental rescans".to_string(),
                    description: Some(
                        "Skip top-level folders whose modified time has not changed since they were indexed. \
                         Only for servers that update a folder's mtime when anything below it changes; \
                         everything is still walked in full once a week."
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
        addon_id: Uuid,
        cfg: &serde_json::Value,
        _config: &crate::Config,
    ) -> Result<AddonCapabilities> {
        let media_kind = cfg["media_kind"]
            .as_str()
            .unwrap_or("movie")
            .to_string();
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("opendal-webdav: endpoint is required"))?;

        let mut builder = opendal::services::Webdav::default().endpoint(endpoint);
        if let Some(u) = cfg["username"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            builder = builder.username(u);
        }
        if let Some(p) = cfg["password"]
            .as_str()
            .filter(|s| !s.is_empty())
        {
            builder = builder.password(p);
        }
        let operator = opendal::Operator::new(builder)?.finish();

        let addon = Arc::new(OpendalAddon {
            addon_id,
            operator: Arc::new(operator),
            root: endpoint.to_string(),
            backend: "webdav".to_string(),
            media_kind,
        });
        Ok(AddonCapabilities {
            kind: Some(addon.clone()),
            catalog: Some(addon.clone()),
            stream: Some(addon.clone()),
            tree: Some(addon.clone()),
            index: Some(addon.clone()),
            subtitle: Some(addon),
            ..Default::default()
        })
    }
}

inventory::submit! {
    AddonPresetRegistration(|| Box::new(OpendalWebdavPreset))
}

// ---------------------------------------------------------------------------
// Shared addon runtime
// ---------------------------------------------------------------------------

pub struct OpendalAddon {
    addon_id: Uuid,
    operator: Arc<opendal::Operator>,
    root: String,
    backend: String,
    media_kind: String,
}

#[derive(sqlx::FromRow)]
pub struct OpendalFile {
    pub path: String,
    pub name: String,
    pub title: Option<String>,
    pub imdb_id: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    pub track_number: Option<i64>,
    pub year: Option<i64>,
    pub size: Option<i64>,
    #[sqlx(default)]
    pub probe_data: Option<sqlx::types::Json<crate::api::MediaSourceInfo>>,
}

#[async_trait]
impl AddonKind for OpendalAddon {
    fn id(&self) -> &'static str {
        "opendal"
    }

    async fn available_info(
        &self,
    ) -> Result<Option<(Vec<remux_sdks::stremio::ResourceRef>, Vec<StremioMediaType>)>>
    {
        let media_type = match self
            .media_kind
            .as_str()
        {
            "episode" => StremioMediaType::Series,
            "track" => StremioMediaType::Track,
            _ => StremioMediaType::Movie,
        };
        let make_ref = |name| remux_sdks::stremio::ResourceRef {
            name,
            types: vec![],
            id_prefixes: None,
        };
        Ok(Some((
            vec![
                make_ref(ResourceType::Stream),
                make_ref(ResourceType::Catalog),
            ],
            vec![media_type],
        )))
    }
}

#[async_trait]
impl CatalogAddon for OpendalAddon {
    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![CatalogInfo {
            provider_catalog_id: "files".to_string(),
            name: "files".to_string(),
            default_enabled: true,
            default_max_items: Some(999999999),
            collection_media_kind: Some(
                self.media_kind
                    .as_str()
                    .into(),
            ),
            media_kind: match self
                .media_kind
                .as_str()
            {
                // episode files are grouped into Series catalog items
                "episode" => Some(db::MediaKind::Series),
                other => other
                    .parse()
                    .ok(),
            },
        }])
    }

    async fn catalog_stream(
        &self,
        ctx: &AppContext,
        local_id: &str,
    ) -> Result<Option<Pin<Box<dyn Stream<Item = db::Media> + Send>>>> {
        if local_id != "files" {
            return Ok(None);
        }

        let items: Vec<db::Media> = match self.media_kind.as_str() {
            "episode" => {
                sqlx::query_as::<_, (String, Option<String>)>(
                    "SELECT DISTINCT imdb_id, title FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'episode' AND imdb_id IS NOT NULL",
                )
                .bind(self.addon_id)
                .fetch_all(&ctx.db)
                .await?
                .into_iter()
                .map(|(imdb_id, title)| db::Media {
                    id: common::get_stable_uuid(format!("series:{}", imdb_id)),
                    title: title.unwrap_or_default(),
                    kind: db::MediaKind::Series,
                    external_ids: db::ExternalIds {
                        imdb: db::NonEmptyString::try_new(imdb_id).ok(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .collect()
            }
            "track" => {
                #[derive(sqlx::FromRow)]
                struct FileRow {
                    path: String,
                    title: Option<String>,
                    track_number: Option<i64>,
                    artist: Option<String>,
                    album: Option<String>,
                    disc_number: Option<i64>,
                }
                let files: Vec<FileRow> = sqlx::query_as(
                    "SELECT path, title, track_number, artist, album, disc_number FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'track' ORDER BY artist, album, disc_number, track_number",
                )
                .bind(self.addon_id)
                .fetch_all(&ctx.db)
                .await?;

                // Jellyfin music hierarchy: Artist -> Album -> Track. Ids are
                // derived from the file path / names so same-titled tracks in
                // different albums never collide and survive reimports.
                let mut seen_artists: std::collections::HashSet<String> =
                    Default::default();
                let mut seen_albums: std::collections::HashSet<String> =
                    Default::default();
                let mut out: Vec<db::Media> = Vec::with_capacity(files.len() * 2);

                for f in &files {
                    let artist_key = f
                        .artist
                        .as_deref()
                        .map(|a| music_artist_key(self.addon_id, a));
                    let album_key = f
                        .album
                        .as_deref()
                        .map(|album| {
                            music_album_key(self.addon_id, f.artist.as_deref(), album)
                        });
                    let artist_id = artist_key
                        .as_ref()
                        .map(|k| common::get_stable_uuid(k.clone()));
                    let album_id = album_key
                        .as_ref()
                        .map(|k| common::get_stable_uuid(k.clone()));
                    if let (Some(artist), Some(key)) = (&f.artist, &artist_key) {
                        if seen_artists.insert(key.clone()) {
                            out.push(db::Media {
                                id: common::get_stable_uuid(key.clone()),
                                title: artist.clone(),
                                kind: db::MediaKind::Artist,
                                external_ids: db::ExternalIds {
                                    custom_stremio_id: Some(key.clone()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });
                        }
                    }
                    if let (Some(album), Some(key)) = (&f.album, &album_key) {
                        if seen_albums.insert(key.clone()) {
                            out.push(db::Media {
                                id: common::get_stable_uuid(key.clone()),
                                title: album.clone(),
                                kind: db::MediaKind::Album,
                                parent_id: artist_id,
                                grandparent_id: artist_id,
                                external_ids: db::ExternalIds {
                                    custom_stremio_id: Some(key.clone()),
                                    ..Default::default()
                                },
                                ..Default::default()
                            });
                        }
                    }
                    let track_key = format!("{}:track:{}", self.addon_id, f.path);
                    let descriptor = if self.backend == "local" {
                        crate::stream::StreamDescriptor::Local(
                            std::path::PathBuf::from(&f.path),
                        )
                    } else {
                        crate::stream::StreamDescriptor::Opendal {
                            addon_id: self.addon_id,
                            path: f
                                .path
                                .clone(),
                        }
                    };
                    out.push(db::Media {
                        id: common::get_stable_uuid(track_key.clone()),
                        title: f
                            .title
                            .clone()
                            .unwrap_or_default(),
                        kind: db::MediaKind::Track,
                        idx: f.track_number,
                        parent_idx: f.disc_number,
                        parent_id: album_id,
                        grandparent_id: artist_id,
                        stream_info: Some(crate::stream::StreamInfo {
                            descriptor,
                            name: Some(f.path.clone()),
                            ..Default::default()
                        }),
                        external_ids: db::ExternalIds {
                            custom_stremio_id: Some(track_key),
                            ..Default::default()
                        },
                        ..Default::default()
                    });
                }
                out
            }
            _ => {
                sqlx::query_as::<_, (String, Option<String>)>(
                    "SELECT DISTINCT imdb_id, title FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'movie' AND imdb_id IS NOT NULL",
                )
                .bind(self.addon_id)
                .fetch_all(&ctx.db)
                .await?
                .into_iter()
                .map(|(imdb_id, title)| db::Media {
                    id: common::get_stable_uuid(format!("movie:{}", imdb_id)),
                    title: title.unwrap_or_default(),
                    kind: db::MediaKind::Movie,
                    external_ids: db::ExternalIds {
                        imdb: db::NonEmptyString::try_new(imdb_id).ok(),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .collect()
            }
        };

        Ok(Some(Box::pin(futures::stream::iter(items))))
    }
}

#[async_trait]
impl IndexAddon for OpendalAddon {
    async fn refresh_index(
        &self,
        ctx: &AppContext,
        addon: &Addon,
        progress: ProgressReporter,
    ) -> Result<()> {
        let tmdb = common::tmdb_client(
            &ctx.db,
            &ctx.config
                .tmdb_base_url,
        )
        .await;
        scan_addon(ctx, &tmdb, addon).await?;
        progress.set(100.0);
        Ok(())
    }

    /// Row count from this addon's last completed scan. `None` when nothing
    /// has been indexed yet (first import) — a genuinely empty library reads
    /// the same as "never scanned" here, which just means it gets the
    /// fallback guess and finishes its slice fast; harmless either way.
    async fn index_estimate(&self, ctx: &AppContext, addon: &Addon) -> Option<usize> {
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM opendal_files WHERE addon_id = ?")
                .bind(addon.id)
                .fetch_one(&ctx.db)
                .await
                .ok()?;
        if count == 0 {
            None
        } else {
            Some(count as usize)
        }
    }

    async fn purge_index(&self, ctx: &AppContext, addon: &Addon) -> Result<()> {
        sqlx::query("DELETE FROM opendal_files WHERE addon_id = ?")
            .bind(addon.id)
            .execute(&ctx.db)
            .await?;
        Ok(())
    }
}

#[async_trait]
impl SubtitleAddon for OpendalAddon {
    fn supports(&self, media: &db::Media) -> bool {
        match self
            .media_kind
            .as_str()
        {
            "movie" => {
                media.kind == db::MediaKind::Movie
                    && media
                        .external_ids
                        .imdb
                        .is_some()
            }
            "episode" => {
                media.kind == db::MediaKind::Episode
                    && media
                        .grandparent
                        .as_deref()
                        .map_or(false, |gp| {
                            gp.external_ids
                                .imdb
                                .is_some()
                        })
            }
            _ => false,
        }
    }

    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        _query: &SubtitleQuery,
        db: &sqlx::SqlitePool,
    ) -> Result<Vec<SubtitleInfo>> {
        let files: Vec<OpendalFile> = if self.media_kind == "episode" {
            let Some(imdb_id) = media
                .grandparent
                .as_deref()
                .and_then(|gp| {
                    gp.external_ids
                        .imdb
                        .as_deref()
                })
            else {
                return Ok(vec![]);
            };
            let season = media
                .parent_idx
                .unwrap_or(0);
            let episode = media
                .idx
                .unwrap_or(0);
            sqlx::query_as(
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size \
                 FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'subtitle' \
                 AND imdb_id = ? AND season = ? AND episode = ?",
            )
            .bind(self.addon_id)
            .bind(imdb_id)
            .bind(season)
            .bind(episode)
            .fetch_all(db)
            .await?
        } else {
            let Some(imdb_id) = media
                .external_ids
                .imdb
                .as_deref()
            else {
                return Ok(vec![]);
            };
            sqlx::query_as(
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size \
                 FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'subtitle' AND imdb_id = ?",
            )
            .bind(self.addon_id)
            .bind(imdb_id)
            .fetch_all(db)
            .await?
        };

        Ok(files
            .into_iter()
            .map(|f| {
                let stem = stem_without_ext(&f.name);
                let (_, lang, is_forced, is_hi) = split_subtitle_stem(&stem);
                SubtitleInfo {
                    id: f
                        .path
                        .clone(),
                    url: Some(crate::stream::StreamDescriptor::Opendal {
                        addon_id: self.addon_id,
                        path: f.path,
                    }),
                    lang,
                    is_forced,
                    is_hi,
                    filename: Some(f.name),
                    from_trusted: None,
                    ai_translated: None,
                }
            })
            .collect())
    }
}

#[async_trait]
impl StreamAddon for OpendalAddon {
    fn supports(&self, media: &db::Media) -> bool {
        match self
            .media_kind
            .as_str()
        {
            "movie" => {
                media.kind == db::MediaKind::Movie
                    && media
                        .external_ids
                        .imdb
                        .is_some()
            }
            "episode" => {
                media.kind == db::MediaKind::Episode
                    && media
                        .grandparent
                        .as_deref()
                        .map_or(false, |gp| {
                            gp.external_ids
                                .imdb
                                .is_some()
                        })
            }
            "track" => media.kind == db::MediaKind::Track,
            _ => false,
        }
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
        _id_prefixes: Option<&[String]>,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let files: Vec<OpendalFile> = if self.media_kind == "track" {
            // Local tracks carry their own descriptor (path) on the media row;
            // fall back to a title+artist+album match for legacy rows.
            // Only reuse it when the file is one this addon indexed: every music
            // addon supports every track, so another addon's descriptor must not
            // be stamped with this addon's identity.
            if let Some(si) = &media.stream_info {
                let owned_path = match &si.descriptor {
                    crate::stream::StreamDescriptor::Local(p) => Some(
                        p.to_string_lossy()
                            .into_owned(),
                    ),
                    crate::stream::StreamDescriptor::Opendal { addon_id, path }
                        if *addon_id == self.addon_id =>
                    {
                        Some(path.clone())
                    }
                    _ => None,
                };
                if let Some(path) = owned_path {
                    let owned: bool = sqlx::query_scalar(
                        "SELECT EXISTS(SELECT 1 FROM opendal_files \
                         WHERE addon_id = ? AND media_kind = 'track' AND path = ?)",
                    )
                    .bind(self.addon_id)
                    .bind(&path)
                    .fetch_one(&ctx.db)
                    .await?;
                    if owned {
                        return Ok(vec![si.clone()]);
                    }
                }
            }
            sqlx::query_as(
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size, probe_data \
                 FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'track' AND LOWER(title) = LOWER(?)",
            )
            .bind(self.addon_id)
            .bind(&media.title)
            .fetch_all(&ctx.db)
            .await?
        } else {
            let imdb_id = if self.media_kind == "episode" {
                media
                    .grandparent
                    .as_deref()
                    .and_then(|gp| {
                        gp.external_ids
                            .imdb
                            .as_deref()
                    })
            } else {
                media
                    .external_ids
                    .imdb
                    .as_deref()
            };
            let imdb_id = match imdb_id {
                Some(id) => id,
                None => return Ok(vec![]),
            };
            sqlx::query_as(
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size, probe_data \
                 FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = ? AND imdb_id = ?",
            )
            .bind(self.addon_id)
            .bind(&self.media_kind)
            .bind(imdb_id)
            .fetch_all(&ctx.db)
            .await?
        };

        let streams = files
            .into_iter()
            .filter(|f| {
                if media.kind == db::MediaKind::Episode {
                    let ep_match = media
                        .idx
                        .map(|e| f.episode == Some(e))
                        .unwrap_or(true);
                    let season_match = media
                        .parent_idx
                        .map(|s| f.season == Some(s))
                        .unwrap_or(true);
                    ep_match && season_match
                } else {
                    true
                }
            })
            .map(|f| {
                let descriptor = if self.backend == "local" {
                    crate::stream::StreamDescriptor::Local(std::path::PathBuf::from(
                        &f.path,
                    ))
                } else {
                    crate::stream::StreamDescriptor::Opendal {
                        addon_id: self.addon_id,
                        path: f
                            .path
                            .clone(),
                    }
                };
                crate::stream::StreamInfo {
                    descriptor,
                    name: Some(
                        f.name
                            .clone(),
                    ),
                    service_cached: Some(true),
                    probe_data: f
                        .probe_data
                        .map(|p| p.0),
                    ..Default::default()
                }
            })
            .collect();

        Ok(streams)
    }

    async fn serve_stream(
        &self,
        descriptor: &crate::stream::StreamDescriptor,
        headers: &axum::http::HeaderMap,
    ) -> axum_anyhow::ApiResult<axum::response::Response> {
        use crate::ResultExt;
        use axum::body::Body;
        use futures_util::TryStreamExt;
        use std::io;

        let path = match descriptor {
            crate::stream::StreamDescriptor::Opendal { path, .. } => path,
            _ => {
                return Err(axum_anyhow::ApiError::builder()
                    .status(axum::http::StatusCode::BAD_REQUEST)
                    .title("stream")
                    .detail("descriptor is not an Opendal path")
                    .build());
            }
        };

        let meta = self
            .operator
            .stat(path)
            .await
            .context_not_found("file not found in opendal backend")?;
        let file_size = meta.content_length();
        let content_type = crate::stream::mime_from_path(std::path::Path::new(path));

        let range_str = headers
            .get(http::header::RANGE)
            .and_then(|v| {
                v.to_str()
                    .ok()
            })
            .map(str::to_owned);

        if let Some(range) = range_str {
            let (start, end) = crate::stream::parse_range(&range, file_size)
                .context_bad_request("invalid Range header")?;
            let length = end - start + 1;

            let reader = self
                .operator
                .reader_with(path)
                .await
                .context_bad_request("failed to open opendal reader")?;
            let bytes_stream = reader
                .into_bytes_stream(start..start + length)
                .await
                .context_bad_request("failed to create opendal byte stream")?
                .map_err(io::Error::other);

            Ok(axum::response::Response::builder()
                .status(http::StatusCode::PARTIAL_CONTENT)
                .header(http::header::CONTENT_TYPE, content_type)
                .header(http::header::CONTENT_LENGTH, length)
                .header(http::header::ACCEPT_RANGES, "bytes")
                .header(
                    http::header::CONTENT_RANGE,
                    format!("bytes {}-{}/{}", start, end, file_size),
                )
                .body(Body::from_stream(bytes_stream))
                .unwrap())
        } else {
            let reader = self
                .operator
                .reader(path)
                .await
                .context_bad_request("failed to open opendal reader")?;
            let bytes_stream = reader
                .into_bytes_stream(..)
                .await
                .context_bad_request("failed to create opendal byte stream")?
                .map_err(io::Error::other);

            Ok(axum::response::Response::builder()
                .status(http::StatusCode::OK)
                .header(http::header::CONTENT_TYPE, content_type)
                .header(http::header::CONTENT_LENGTH, file_size)
                .header(http::header::ACCEPT_RANGES, "bytes")
                .body(Body::from_stream(bytes_stream))
                .unwrap())
        }
    }
}

#[async_trait]
impl TreeAddon for OpendalAddon {
    fn supports(&self, root: &db::Media) -> bool {
        self.media_kind == "episode"
            && matches!(root.kind, db::MediaKind::Series | db::MediaKind::Season)
    }

    async fn get_children(
        &self,
        root: &db::Media,
        ctx: &AppContext,
    ) -> Result<Option<Vec<db::Media>>> {
        if self.media_kind != "episode" {
            return Ok(None);
        }

        match root.kind {
            db::MediaKind::Series => {
                let Some(imdb_id) = root
                    .external_ids
                    .imdb
                    .as_deref()
                else {
                    return Ok(None);
                };
                let season_nums: Vec<i64> = sqlx::query_scalar(
                    "SELECT DISTINCT season FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'episode' \
                       AND imdb_id = ? AND season IS NOT NULL \
                     ORDER BY season",
                )
                .bind(self.addon_id)
                .bind(imdb_id)
                .fetch_all(&ctx.db)
                .await?;

                if season_nums.is_empty() {
                    return Ok(None);
                }

                let gp_box = Some(Arc::new(root.clone()));
                let seasons = season_nums
                    .into_iter()
                    .map(|s| db::Media {
                        id: common::get_stable_uuid(format!(
                            "season:{}:{}",
                            imdb_id, s
                        )),
                        title: crate::addons::season_title(s),
                        kind: db::MediaKind::Season,
                        parent_id: Some(root.id),
                        grandparent_id: Some(root.id),
                        idx: Some(s),
                        parent_idx: Some(s),
                        grandparent: gp_box.clone(),
                        ..Default::default()
                    })
                    .collect();

                Ok(Some(seasons))
            }

            db::MediaKind::Season => {
                // Resolve grandparent (Series) — try in-memory first, then DB.
                // Kept as one shared `Arc`, not cloned into an owned `Media` per
                // episode below — this stub can carry embedded relations, and a
                // deep clone per episode is exactly the multiplication that made
                // large-tree refreshes memory-heavy elsewhere (see `Media::parent`).
                let gp: Option<Arc<db::Media>> = if let Some(gp) = root
                    .grandparent
                    .clone()
                {
                    Some(gp)
                } else if let Some(gp_id) = root
                    .grandparent_id
                    .or(root.parent_id)
                {
                    db::Media::get_by_id(&ctx.db, &gp_id)
                        .await
                        .ok()
                        .flatten()
                        .map(Arc::new)
                } else {
                    None
                };
                let Some(gp) = gp else {
                    return Ok(None);
                };
                let Some(series_imdb) = gp
                    .external_ids
                    .imdb
                    .as_deref()
                else {
                    return Ok(None);
                };
                let Some(season_num) = root.idx else {
                    return Ok(None);
                };
                let series_id = root
                    .parent_id
                    .unwrap_or(root.id);

                let files: Vec<OpendalFile> = sqlx::query_as(
                    "SELECT path, name, title, imdb_id, season, episode, \
                            track_number, year, size \
                     FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'episode' \
                       AND imdb_id = ? AND season = ? \
                     ORDER BY episode",
                )
                .bind(self.addon_id)
                .bind(series_imdb)
                .bind(season_num)
                .fetch_all(&ctx.db)
                .await?;

                if files.is_empty() {
                    return Ok(None);
                }

                let episodes: Vec<db::Media> = files
                    .into_iter()
                    .filter_map(|f| {
                        let ep_num = f.episode?;
                        // Leave title empty so the TMDB meta addon can fill in the proper
                        // episode name via refresh_meta (which apply_title_format then wraps).
                        let title = String::new();
                        let descriptor = if self.backend == "local" {
                            crate::stream::StreamDescriptor::Local(
                                std::path::PathBuf::from(&f.path),
                            )
                        } else {
                            crate::stream::StreamDescriptor::Opendal {
                                addon_id: self.addon_id,
                                path: f
                                    .path
                                    .clone(),
                            }
                        };
                        Some(db::Media {
                            id: common::get_stable_uuid(format!(
                                "episode:{}:{}:{}",
                                series_imdb, season_num, ep_num
                            )),
                            title,
                            kind: db::MediaKind::Episode,
                            parent_id: Some(root.id),
                            grandparent_id: Some(series_id),
                            grandparent: Some(gp.clone()),
                            idx: Some(ep_num),
                            parent_idx: Some(season_num),
                            stream_info: Some(crate::stream::StreamInfo {
                                descriptor,
                                name: Some(
                                    f.name
                                        .clone(),
                                ),
                                service_cached: Some(true),
                                ..Default::default()
                            }),
                            ..Default::default()
                        })
                    })
                    .collect();

                Ok(Some(episodes))
            }

            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Opendal file index scanning (backing refresh_index)
// ---------------------------------------------------------------------------

const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "sup"];

/// Extract the file stem (filename without the last extension).
fn stem_without_ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => name.to_string(),
    }
}

fn movie_title_from_stem(
    stem: &str,
    parsed_title: Option<&str>,
    year: Option<i64>,
) -> String {
    // Jellyfin treats the complete text before a parenthesized year as the
    // movie title. Its multipart rules only match numeric/a-d suffixes at the
    // end of a filename, so a title such as "Dune Part Two (2024)" must retain
    // "Part Two" even though hunch classifies it as Part=2.
    if let (Some(parsed_title), Some(year)) = (parsed_title, year) {
        let year_marker = format!("({year})");
        if let Some(prefix) = stem
            .split_once(&year_marker)
            .map(|(prefix, _)| prefix.trim())
            .filter(|prefix| prefix.len() > parsed_title.len())
            .filter(|prefix| prefix.starts_with(parsed_title))
        {
            return prefix.to_string();
        }
    }

    parsed_title
        .unwrap_or(stem)
        .to_string()
}

/// Split a subtitle stem (filename without its subtitle extension) into its base and subtitle metadata.
///
/// For `Breaking.Bad.S01E01.en.forced` returns `("Breaking.Bad.S01E01", Some("en"), true, false)`.
/// Scans right-to-left: known flags first, then a 2–3-letter lang code, remainder is the base.
fn split_subtitle_stem(stem: &str) -> (String, Option<String>, bool, bool) {
    let parts: Vec<&str> = stem
        .split('.')
        .collect();
    if parts.is_empty() {
        return (stem.to_string(), None, false, false);
    }
    let mut suffix_start = parts.len();
    let mut is_forced = false;
    let mut is_hi = false;

    while suffix_start > 0 {
        match parts[suffix_start - 1]
            .to_ascii_lowercase()
            .as_str()
        {
            "forced" => {
                is_forced = true;
                suffix_start -= 1;
            }
            "hi" | "sdh" | "cc" => {
                is_hi = true;
                suffix_start -= 1;
            }
            "default" => {
                suffix_start -= 1;
            }
            _ => break,
        }
    }

    let lang = if suffix_start > 0 {
        let part = parts[suffix_start - 1];
        if part.len() >= 2
            && part.len() <= 3
            && part
                .chars()
                .all(|c| c.is_ascii_alphabetic())
        {
            suffix_start -= 1;
            Some(part.to_string())
        } else {
            None
        }
    } else {
        None
    };

    (parts[..suffix_start].join("."), lang, is_forced, is_hi)
}

/// Stable key of a local music artist; the id and `custom_stremio_id` derive
/// from it.
fn music_artist_key(addon_id: Uuid, artist: &str) -> String {
    format!("{addon_id}:artist:{artist}")
}

/// Stable key of a local music album. The artist's length comes first so names
/// containing `:` can't make two different artist/album pairs share a key
/// (`A:B` + `C` versus `A` + `B:C`).
fn music_album_key(addon_id: Uuid, artist: Option<&str>, album: &str) -> String {
    let artist = artist.unwrap_or("");
    format!("{addon_id}:album:{}:{artist}:{album}", artist.len())
}

/// Artist, album and disc number of a music file from the Jellyfin folder
/// layout: `{Artist}/{Album}/{NN - Title}`, album-only folders (`{Album}/{Track}`)
/// and loose tracks. A `Disc N` / `CD N` folder inside the album carries the
/// disc number instead of naming the album. `rel_path` is relative to the
/// operator root; `scan_dir` is the configured directory it was listed from
/// (`/` for local roots) and is not part of the layout.
fn music_layout(
    rel_path: &str,
    scan_dir: &str,
) -> (Option<String>, Option<String>, Option<i64>) {
    let rel = rel_path.trim_matches('/');
    let base = scan_dir.trim_matches('/');
    let rel = if base.is_empty() {
        rel
    } else {
        rel.strip_prefix(base)
            .map(|r| r.trim_start_matches('/'))
            .unwrap_or(rel)
    };
    let mut dirs: Vec<&str> = rel
        .split('/')
        .filter(|c| !c.is_empty())
        .collect();
    dirs.pop(); // the file itself

    static DISC_DIR: std::sync::LazyLock<Regex> = std::sync::LazyLock::new(|| {
        Regex::new(r"(?i)^(?:disc|disk|cd)[\s._-]*(\d{1,2})$").unwrap()
    });
    let mut disc = None;
    if let Some(n) = dirs
        .last()
        .and_then(|d| DISC_DIR.captures(d))
        .and_then(|c| {
            c[1].parse::<i64>()
                .ok()
        })
    {
        disc = Some(n);
        dirs.pop();
    }
    match dirs.as_slice() {
        [artist, album, ..] => (
            Some((*artist).to_string()),
            Some((*album).to_string()),
            disc,
        ),
        [album] => (None, Some((*album).to_string()), disc),
        _ => (None, None, disc),
    }
}

async fn scan_addon(
    ctx: &AppContext,
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    addon: &Addon,
) -> Result<()> {
    let cfg = addon
        .preset
        .config
        .expose();
    let media_kind = cfg["media_kind"]
        .as_str()
        .unwrap_or("movie")
        .to_string();
    let is_local = addon
        .preset
        .kind
        == "opendal-local";

    info!(addon = %addon.name, kind = %addon.preset.kind, media_kind, "opendal: scanning");

    let is_media_ext: fn(&str) -> bool = if media_kind == "track" {
        |ext| {
            ext == "strm"
                || remux_sdks::remux::AudioContainer::parse_known(ext).is_some()
        }
    } else {
        |ext| {
            ext == "strm"
                || remux_sdks::remux::VideoContainer::parse_known(ext).is_some()
        }
    };

    let track_num_re = Regex::new(r"^(\d{1,3})[.\s\-_\[\]]+").unwrap();

    // Build (operator, list_from, path_prefix) for each configured path.
    // Local: one Fs operator per root, list from "/", prefix gives absolute stored path.
    // WebDAV: one shared operator, list from each sub-path, no prefix needed.
    let scan_roots: Vec<(opendal::Operator, String, String)> = if is_local {
        cfg_paths_local(cfg)?
            .into_iter()
            .map(|p| {
                let op =
                    opendal::Operator::new(opendal::services::Fs::default().root(&p))?
                        .finish();
                Ok((op, "/".to_string(), p))
            })
            .collect::<Result<_>>()?
    } else {
        let op = build_webdav_operator(cfg)?;
        cfg_paths_webdav(cfg)
            .into_iter()
            .map(|p| (op.clone(), p, String::new()))
            .collect()
    };

    let mut seen_ids: Vec<Uuid> = Vec::new();
    let mut upserted = 0usize;

    let probe_on_scan = addon.probe_on_scan && media_kind != "track";
    // With probing off, existing probe data is kept, but it must still be dropped
    // when its file changes, so changes are tracked while any probe data remains.
    let track_probes = probe_on_scan
        || sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM opendal_files WHERE addon_id = ? AND probe_data IS NOT NULL)",
        )
        .bind(addon.id)
        .fetch_one(&ctx.db)
        .await?;
    let known_probes: std::collections::HashMap<Uuid, (Option<String>, bool)> =
        if track_probes {
            sqlx::query_as::<_, (Uuid, Option<String>, bool)>(
                "SELECT id, probe_version, probe_data IS NOT NULL FROM opendal_files WHERE addon_id = ?",
            )
            .bind(addon.id)
            .fetch_all(&ctx.db)
            .await?
            .into_iter()
            .map(|(id, version, has)| (id, (version, has)))
            .collect()
        } else {
            Default::default()
        };
    let webdav_probe = if probe_on_scan && !is_local {
        WebdavProbeBase::from_cfg(cfg)
    } else {
        None
    };
    let mut probe_jobs: Vec<ProbeJob> = Vec::new();
    let mut path_id_memo = PathIdMemo::default();

    // Debrid-style WebDAV shares only ever add folders, so a folder that has not
    // changed since it was indexed is not walked again. This is opt-in
    // (`incremental_scan: true` in the addon config): many servers only bump a
    // directory's mtime for direct children, so a change deep in a tree may not be
    // seen until the weekly full walk. Local scans never skip.
    let incremental = if is_local || cfg["incremental_scan"].as_bool() != Some(true) {
        None
    } else {
        IncrementalIndex::load(ctx, addon.id).await?
    };

    for (operator, list_from, path_prefix) in scan_roots {
        let (mut single, mut lister) = open_scan_source(
            &operator,
            &list_from,
            is_local,
            incremental.as_ref(),
            &mut seen_ids,
        )
        .await?;

        while let Some(entry) = next_scan_entry(&mut single, &mut lister).await? {
            // The lister classifies entries from a raw readdir() call, which does not
            // follow symlinks and reports them as EntryMode::Unknown. Resolve those
            // via stat() (which does follow symlinks) so symlinked media files staged
            // by tools like Sonarr/Radarr + a debrid manager are indexed correctly.
            let mode = entry
                .metadata()
                .mode();
            let is_file = mode == EntryMode::FILE
                || (mode == EntryMode::Unknown
                    && operator
                        .stat(entry.path())
                        .await
                        .map(|m| m.mode() == EntryMode::FILE)
                        .unwrap_or(false));
            if !is_file {
                continue;
            }

            let entry_rel = entry
                .path()
                .to_string();
            let path = if path_prefix.is_empty() {
                entry_rel.clone()
            } else {
                format!(
                    "{}/{}",
                    path_prefix.trim_end_matches('/'),
                    entry_rel.trim_start_matches('/')
                )
            };
            let name = entry
                .name()
                .to_string();
            let ext = std::path::Path::new(&name)
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or("")
                .to_lowercase();

            // Subtitle files: parse IMDB from filename (same convention as video files).
            if SUBTITLE_EXTENSIONS.contains(&ext.as_str()) {
                let stem = stem_without_ext(&name);
                let (base_stem, _, _, _) = split_subtitle_stem(&stem);
                let jellyfin_ids = db::ExternalIds::from_path(&path);
                let parsed = hunch::hunch(&base_stem);

                let (imdb_id, season, episode, year, title_str) =
                    match media_kind.as_str() {
                        "episode" => {
                            let season = parsed
                                .season()
                                .map(|s| s as i64);
                            let episode = parsed
                                .episode()
                                .map(|e| e as i64);
                            let year = parsed
                                .year()
                                .map(|y| y as i64);
                            let clean_title = parsed
                                .title()
                                .unwrap_or(base_stem.as_str())
                                .to_string();
                            let existing_imdb =
                                fetch_existing_imdb(ctx, addon.id, &path).await?;
                            let imdb_id = if let Some(id) = existing_imdb {
                                Some(id)
                            } else if !jellyfin_ids.is_empty() {
                                if let Some(client) = tmdb {
                                    resolve_path_ids(
                                        &mut path_id_memo,
                                        &jellyfin_ids,
                                        true,
                                        client,
                                    )
                                    .await
                                } else {
                                    jellyfin_ids
                                        .imdb
                                        .clone()
                                        .map(Into::into)
                                }
                            } else {
                                if let Some(client) = tmdb {
                                    MediaResolveService::resolve_imdb_from_search(
                                        client,
                                        &clean_title,
                                        None,
                                        true,
                                    )
                                    .await
                                    .map(Into::into)
                                } else {
                                    None
                                }
                            };
                            (imdb_id, season, episode, year, clean_title)
                        }
                        _ => {
                            let year = parsed
                                .year()
                                .map(|y| y as i64);
                            let clean_title = parsed
                                .title()
                                .unwrap_or(base_stem.as_str())
                                .to_string();
                            let existing_imdb =
                                fetch_existing_imdb(ctx, addon.id, &path).await?;
                            let imdb_id = if let Some(id) = existing_imdb {
                                Some(id)
                            } else if !jellyfin_ids.is_empty() {
                                if let Some(client) = tmdb {
                                    resolve_path_ids(
                                        &mut path_id_memo,
                                        &jellyfin_ids,
                                        false,
                                        client,
                                    )
                                    .await
                                } else {
                                    jellyfin_ids
                                        .imdb
                                        .clone()
                                        .map(Into::into)
                                }
                            } else {
                                if let Some(client) = tmdb {
                                    MediaResolveService::resolve_imdb_from_search(
                                        client,
                                        &clean_title,
                                        year,
                                        false,
                                    )
                                    .await
                                    .map(Into::into)
                                } else {
                                    None
                                }
                            };
                            (imdb_id, None, None, year, clean_title)
                        }
                    };

                if imdb_id.is_none() {
                    debug!(path, "opendal: subtitle has no IMDB id, skipping");
                    continue;
                }

                let sub_id = common::get_stable_uuid(format!("{}:{}", addon.id, path));
                seen_ids.push(sub_id);
                let now = Utc::now()
                    .naive_utc()
                    .to_string();
                sqlx::query(
                    "INSERT INTO opendal_files \
                     (id, addon_id, media_kind, path, name, title, imdb_id, season, episode, track_number, year, size, scanned_at) \
                     VALUES (?, ?, 'subtitle', ?, ?, ?, ?, ?, ?, NULL, ?, NULL, ?) \
                     ON CONFLICT(id) DO UPDATE SET \
                       path = excluded.path, name = excluded.name, \
                       title = excluded.title, \
                       imdb_id = COALESCE(opendal_files.imdb_id, excluded.imdb_id), \
                       season = excluded.season, episode = excluded.episode, \
                       year = excluded.year, scanned_at = excluded.scanned_at",
                )
                .bind(sub_id)
                .bind(addon.id)
                .bind(&path)
                .bind(&name)
                .bind(&title_str)
                .bind(imdb_id.as_deref())
                .bind(season)
                .bind(episode)
                .bind(year)
                .bind(&now)
                .execute(&ctx.db)
                .await?;

                debug!(path, "opendal: indexed subtitle");
                upserted += 1;
                continue;
            }

            if !is_media_ext(ext.as_str()) {
                continue;
            }

            // Skip files inside special-feature subdirectories or with extra-file names.
            // Aligned with Jellyfin's NamingOptions.VideoExtraRules.
            const SKIP_DIRS: &[&str] = &[
                "trailers",
                "trailer",
                "backdrops",
                "behind the scenes",
                "deleted scenes",
                "interviews",
                "interview",
                "scenes",
                "samples",
                "shorts",
                "featurettes",
                "featurette",
                "extras",
                "extra",
                "other",
                "clips",
                "specials",
            ];
            const SKIP_STEMS: &[&str] = &["trailer", "sample", "theme"];
            const SKIP_SUFFIXES: &[&str] = &[
                "-trailer",
                ".trailer",
                "_trailer",
                "- trailer",
                "-sample",
                ".sample",
                "_sample",
                "- sample",
                "-scene",
                "-clip",
                "-interview",
                "-behindthescenes",
                "-deleted",
                "-deletedscene",
                "-featurette",
                "-short",
                "-extra",
                "-other",
            ];
            let path_components: Vec<&str> = entry_rel
                .trim_end_matches('/')
                .split('/')
                .collect();
            let dir_components = path_components
                .len()
                .saturating_sub(1);
            if path_components[..dir_components]
                .iter()
                .any(|c| {
                    let lower = c.to_lowercase();
                    let normalized = lower.trim_matches(|ch: char| {
                        ch == '.' || ch == '[' || ch == ']' || ch.is_whitespace()
                    });
                    SKIP_DIRS
                        .iter()
                        .any(|s| normalized == *s)
                })
            {
                debug!(path, "opendal: skipping file in special-feature subdir");
                continue;
            }
            let stem_lower = std::path::Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_lowercase();
            if SKIP_STEMS.contains(&stem_lower.as_str())
                || SKIP_SUFFIXES
                    .iter()
                    .any(|s| stem_lower.ends_with(s))
            {
                debug!(path, "opendal: skipping extra file by filename");
                continue;
            }

            let row_id = common::get_stable_uuid(format!("{}:{}", addon.id, path));
            seen_ids.push(row_id);

            let stored_path: String = if ext == "strm" {
                match operator
                    .read(&entry_rel)
                    .await
                {
                    Ok(buf) => {
                        let url = String::from_utf8_lossy(&buf.to_bytes())
                            .trim()
                            .to_string();
                        if url.is_empty() {
                            warn!(path, "opendal: empty strm file, skipping");
                            continue;
                        }
                        url
                    }
                    Err(e) => {
                        warn!(path, error = %e, "opendal: failed to read strm, skipping");
                        continue;
                    }
                }
            } else {
                path.clone()
            };

            let stem = std::path::Path::new(&name)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(&name)
                .to_string();

            let jellyfin_ids = db::ExternalIds::from_path(&path);

            let (title, season, episode, track_number, year, imdb_id) = match media_kind
                .as_str()
            {
                "track" => {
                    let track_number = track_num_re
                        .captures(&stem)
                        .and_then(|c| c.get(1))
                        .and_then(|m| {
                            m.as_str()
                                .parse::<i64>()
                                .ok()
                        });
                    let clean_stem = if track_number.is_some() {
                        track_num_re
                            .replace(&stem, "")
                            .into_owned()
                    } else {
                        stem.clone()
                    };
                    let parsed = hunch::hunch(&clean_stem);
                    let title = parsed
                        .title()
                        .unwrap_or(clean_stem.as_str())
                        .to_string();
                    (Some(title), None, None, track_number, None, None)
                }
                "episode" => {
                    let parsed = hunch::hunch(&stem);
                    let season = parsed
                        .season()
                        .map(|s| s as i64);
                    let episode = parsed
                        .episode()
                        .map(|e| e as i64);

                    // When the filename starts with the episode code (e.g. "S01E07 - Title"),
                    // hunch finds no title before it and returns None. In that case the series
                    // folder name is the authoritative source; using the stem would store the
                    // episode title (or the whole filename) as the series name.
                    let (clean_title, year) = match parsed
                        .title()
                        .filter(|t| !t.is_empty())
                    {
                        Some(t) => {
                            let year = parsed
                                .year()
                                .map(|y| y as i64);
                            (t.to_string(), year)
                        }
                        None if path_components.len() >= 2 => {
                            let series_dir = path_components[0];
                            let dir_parsed = hunch::hunch(series_dir);
                            let title = dir_parsed
                                .title()
                                .filter(|t| !t.is_empty())
                                .unwrap_or(series_dir)
                                .to_string();
                            let year = dir_parsed
                                .year()
                                .map(|y| y as i64);
                            (title, year)
                        }
                        _ => {
                            let year = parsed
                                .year()
                                .map(|y| y as i64);
                            (stem.clone(), year)
                        }
                    };

                    let existing_imdb =
                        fetch_existing_imdb(ctx, addon.id, &stored_path).await?;
                    let imdb_id = if let Some(id) = existing_imdb {
                        Some(id)
                    } else if !jellyfin_ids.is_empty() {
                        if let Some(client) = tmdb {
                            resolve_path_ids(
                                &mut path_id_memo,
                                &jellyfin_ids,
                                true,
                                client,
                            )
                            .await
                        } else {
                            jellyfin_ids
                                .imdb
                                .clone()
                                .map(Into::into)
                        }
                    } else {
                        if let Some(client) = tmdb {
                            MediaResolveService::resolve_imdb_from_search(
                                client,
                                &clean_title,
                                None,
                                true,
                            )
                            .await
                            .map(Into::into)
                        } else {
                            None
                        }
                    };

                    if imdb_id.is_none() {
                        debug!(path, title = %clean_title, "opendal: no IMDB id, skipping");
                        continue;
                    }

                    (Some(clean_title), season, episode, None, year, imdb_id)
                }
                _ => {
                    // movie
                    let parsed = hunch::hunch(&stem);
                    let year = parsed
                        .year()
                        .map(|y| y as i64);
                    let clean_title =
                        movie_title_from_stem(&stem, parsed.title(), year);

                    let existing_imdb =
                        fetch_existing_imdb(ctx, addon.id, &stored_path).await?;
                    let imdb_id = if let Some(id) = existing_imdb {
                        Some(id)
                    } else if !jellyfin_ids.is_empty() {
                        if let Some(client) = tmdb {
                            resolve_path_ids(
                                &mut path_id_memo,
                                &jellyfin_ids,
                                false,
                                client,
                            )
                            .await
                        } else {
                            jellyfin_ids
                                .imdb
                                .clone()
                                .map(Into::into)
                        }
                    } else {
                        if let Some(client) = tmdb {
                            MediaResolveService::resolve_imdb_from_search(
                                client,
                                &clean_title,
                                year,
                                false,
                            )
                            .await
                            .map(Into::into)
                        } else {
                            None
                        }
                    };

                    if imdb_id.is_none() {
                        debug!(path, title = %clean_title, "opendal: no IMDB id, skipping");
                        continue;
                    }

                    (Some(clean_title), None, None, None, year, imdb_id)
                }
            };

            // Jellyfin music layout, see `music_layout`. Without embedded tags the
            // folder names are the source of truth for artist/album.
            let (artist, album, disc_number) = if media_kind == "track" {
                music_layout(&entry_rel, &list_from)
            } else {
                (None, None, None)
            };

            // The local lister reports neither length nor mtime, so probing falls
            // back to a stat to detect changed files.
            let mut meta = entry
                .metadata()
                .clone();
            if track_probes
                && (meta.content_length() == 0
                    || (meta
                        .last_modified()
                        .is_none()
                        && meta
                            .etag()
                            .is_none()))
            {
                if let Ok(statted) = operator
                    .stat(entry.path())
                    .await
                {
                    meta = statted;
                }
            }
            let size = Some(meta.content_length() as i64);
            // For `.strm` files `stored_path` is the URL, so a retargeted file
            // changes the version even when its size doesn't.
            let probe_version =
                track_probes.then(|| probe_version_of(&stored_path, &meta));
            let now = Utc::now()
                .naive_utc()
                .to_string();

            let insert_result = sqlx::query(
                "INSERT INTO opendal_files \
                 (id, addon_id, media_kind, path, name, title, imdb_id, season, episode, track_number, year, size, scanned_at, artist, album, disc_number) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET \
                   path = excluded.path, \
                   name = excluded.name, media_kind = excluded.media_kind, \
                   title = excluded.title, \
                   imdb_id = COALESCE(opendal_files.imdb_id, excluded.imdb_id), \
                   season = excluded.season, episode = excluded.episode, \
                   track_number = excluded.track_number, \
                   year = excluded.year, size = excluded.size, scanned_at = excluded.scanned_at, \
                   artist = excluded.artist, album = excluded.album, \
                   disc_number = excluded.disc_number, \
                   probe_data = CASE WHEN ? IS NOT NULL AND ? IS NOT opendal_files.probe_version THEN NULL ELSE opendal_files.probe_data END",
            )
            .bind(row_id)
            .bind(addon.id)
            .bind(&media_kind)
            .bind(&stored_path)
            .bind(&name)
            .bind(title.as_deref())
            .bind(imdb_id.as_deref())
            .bind(season)
            .bind(episode)
            .bind(track_number)
            .bind(year)
            .bind(size)
            .bind(&now)
            .bind(artist.as_deref())
            .bind(album.as_deref())
            .bind(disc_number)
            .bind(probe_version.as_deref())
            .bind(probe_version.as_deref())
            .execute(&ctx.db)
            .await;

            // A UNIQUE(addon_id, path) clash here means some other row (a different
            // id) already holds this path — typically a stale entry from a file that
            // has since been renamed/regenerated (e.g. a `.strm` whose URL rotated).
            // `id` and `path` come from different sources for `.strm` entries (fs path
            // vs. the URL read from the file), so `ON CONFLICT(id)` can't reconcile it.
            // Skip only that specific violation instead of aborting the whole scan via
            // `?` — bailing out here would also skip `prune_stale_paths` below, so the
            // stale row would never get cleaned up and every future scan would hit the
            // same clash again. Any other error (connection loss, disk I/O, etc.) is
            // still propagated — swallowing those could make the scan report success
            // while `prune_stale_paths` deletes rows based on an incomplete `seen_ids`.
            match insert_result {
                Ok(_) => {
                    upserted += 1;
                    let unchanged = known_probes
                        .get(&row_id)
                        .is_some_and(|(old_version, has_probe)| {
                            *has_probe && *old_version == probe_version
                        });
                    if let Some(version) =
                        probe_version.filter(|_| probe_on_scan && !unchanged)
                    {
                        let target = if stored_path.starts_with("http://")
                            || stored_path.starts_with("https://")
                        {
                            Some(ProbeJob {
                                row_id,
                                input: stored_path
                                    .clone()
                                    .into(),
                                headers: None,
                                version: version.clone(),
                            })
                        } else if is_local {
                            Some(ProbeJob {
                                row_id,
                                input: stored_path
                                    .clone()
                                    .into(),
                                headers: None,
                                version: version.clone(),
                            })
                        } else {
                            webdav_probe
                                .as_ref()
                                .map(|base| base.job(row_id, &entry_rel, version))
                        };
                        probe_jobs.extend(target);
                    }
                }
                Err(sqlx::Error::Database(e)) if e.is_unique_violation() => {
                    warn!(
                        path = %stored_path,
                        error = %e,
                        "opendal: skipping file due to UNIQUE(addon_id, path) collision"
                    );
                }
                Err(e) => return Err(e.into()),
            }
        }
    }

    let deleted = prune_stale_paths(ctx, addon.id, &seen_ids).await?;
    run_probe_jobs(ctx, addon, probe_jobs).await;

    info!(
        addon = %addon.name,
        upserted,
        deleted,
        "opendal: scan complete"
    );

    Ok(())
}

/// Identifies the exact file content a probe was taken from. For `.strm` files
/// `stored_path` is the target URL, so retargeting changes it even when the
/// size doesn't.
fn probe_version_of(stored_path: &str, meta: &opendal::Metadata) -> String {
    format!(
        "{stored_path}|{}|{:?}|{}",
        meta.content_length(),
        meta.last_modified(),
        meta.etag()
            .unwrap_or("")
    )
}

const PROBE_CONCURRENCY: usize = 4;

struct ProbeJob {
    row_id: Uuid,
    input: remux_utils::Secret<String>,
    headers: Option<String>,
    version: String,
}

struct WebdavProbeBase {
    endpoint: String,
    auth_header: Option<String>,
}

impl WebdavProbeBase {
    fn from_cfg(cfg: &serde_json::Value) -> Option<Self> {
        use base64::Engine;
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|s| s.starts_with("http://") || s.starts_with("https://"))?
            .trim_end_matches('/')
            .to_string();
        let username = cfg["username"]
            .as_str()
            .filter(|s| !s.is_empty());
        let password = cfg["password"]
            .as_str()
            .unwrap_or("");
        let auth_header = username.map(|u| {
            let token = base64::engine::general_purpose::STANDARD
                .encode(format!("{u}:{password}"));
            format!("Authorization: Basic {token}\r\n")
        });
        Some(Self {
            endpoint,
            auth_header,
        })
    }

    fn job(&self, row_id: Uuid, rel_path: &str, version: String) -> ProbeJob {
        let encoded = rel_path
            .trim_start_matches('/')
            .split('/')
            .map(|seg| urlencoding::encode(seg).into_owned())
            .collect::<Vec<_>>()
            .join("/");
        ProbeJob {
            row_id,
            input: format!("{}/{}", self.endpoint, encoded).into(),
            headers: self
                .auth_header
                .clone(),
            version,
        }
    }
}

/// Probes the queued files a few at a time and stores each result on its
/// `opendal_files` row. A failed probe is only logged: the row keeps no
/// probe data and playback probes it on demand as before.
async fn run_probe_jobs(ctx: &AppContext, addon: &Addon, jobs: Vec<ProbeJob>) {
    use futures::StreamExt;
    if jobs.is_empty() {
        return;
    }
    info!(addon = %addon.name, files = jobs.len(), "opendal: probing files");
    let probe_timeout = Duration::from_secs(
        db::Settings::get_config_or_default(&ctx.db)
            .await
            .probe_timeout_secs
            .unwrap_or(20) as u64,
    );
    let mut stored = 0usize;
    let mut results = futures::stream::iter(jobs)
        .map(|job| async move {
            let handle = tokio::task::spawn_blocking(move || {
                crate::playback::probe::probe_media_with(
                    job.input
                        .expose(),
                    job.headers
                        .as_deref(),
                    Some(probe_timeout),
                )
            });
            let probed = match tokio::time::timeout(probe_timeout, handle).await {
                Ok(Ok(Ok((mut info, segments)))) => {
                    if info
                        .video_stream()
                        .is_none()
                        && info
                            .audio_stream()
                            .is_none()
                    {
                        None
                    } else {
                        if !segments.is_empty() {
                            info.segments = Some(segments);
                        }
                        Some(info)
                    }
                }
                Ok(Ok(Err(e))) => {
                    debug!(error = %e, "opendal: probe failed");
                    None
                }
                Ok(Err(e)) => {
                    warn!(error = %e, "opendal: probe task panicked");
                    None
                }
                Err(_) => {
                    debug!("opendal: probe timed out");
                    None
                }
            };
            (job.row_id, job.version, probed)
        })
        .buffer_unordered(PROBE_CONCURRENCY);
    while let Some((row_id, version, probed)) = results
        .next()
        .await
    {
        let Some(info) = probed else { continue };
        match sqlx::query(
            "UPDATE opendal_files SET probe_data = ?, probe_version = ? WHERE id = ?",
        )
        .bind(sqlx::types::Json(&info))
        .bind(version)
        .bind(row_id)
        .execute(&ctx.db)
        .await
        {
            Ok(_) => stored += 1,
            Err(e) => warn!(error = %e, "opendal: failed to store probe data"),
        }
    }
    info!(addon = %addon.name, stored, "opendal: probing complete");
}

/// Directory form of a configured WebDAV path (always with a trailing slash).
///
/// opendal's WebDAV backend cannot list recursively on its own, so it falls back to a
/// client-side walk. For a path *without* a trailing slash that walk starts from the
/// path's parent and only prefix-filters the results afterwards, which for a top-level
/// folder means crawling the entire share once per configured path.
fn webdav_dir_path(path: &str) -> String {
    let trimmed = path.trim_matches('/');
    if trimmed.is_empty() {
        "/".to_string()
    } else {
        format!("/{trimmed}/")
    }
}

/// An entry produced by a scan: either from a lister or a path that is a single file.
struct ScanEntry {
    path: String,
    metadata: opendal::Metadata,
}

impl From<opendal::Entry> for ScanEntry {
    fn from(entry: opendal::Entry) -> Self {
        let (path, metadata) = entry.into_parts();
        Self { path, metadata }
    }
}

impl ScanEntry {
    fn path(&self) -> &str {
        &self.path
    }

    fn name(&self) -> &str {
        self.path
            .trim_end_matches('/')
            .rsplit('/')
            .next()
            .unwrap_or_default()
    }

    fn metadata(&self) -> &opendal::Metadata {
        &self.metadata
    }
}

/// How far before the newest indexed row a folder's mtime must fall to count as
/// unchanged. Covers a change landing between listing a folder and writing its
/// rows, and clock skew between this server and the share.
const INCREMENTAL_SLACK: chrono::Duration = chrono::Duration::hours(1);

/// A full walk is forced when the stalest row was last seen longer ago than this.
const FULL_WALK_INTERVAL: chrono::Duration = chrono::Duration::days(7);

/// One indexed row, as far as the skip decision is concerned.
struct IndexedRow {
    id: Uuid,
    path: String,
    scanned_at: chrono::NaiveDateTime,
    /// A non-track row that never got an IMDB id.
    unresolved: bool,
}

/// What a previous scan stored, used to decide which top-level folders to skip.
struct IncrementalIndex {
    /// Sorted by stored path, so the rows below one folder are a contiguous slice.
    rows: Vec<IndexedRow>,
}

impl IncrementalIndex {
    /// `None` when skipping is not safe or not wanted for this addon:
    /// - rows from `.strm` files store the target URL as their path, so they cannot
    ///   be attributed to a folder;
    /// - the last full walk is too old. Many servers only bump a directory's mtime
    ///   for its direct children, so a change deep in a tree can go unseen; a
    ///   periodic full walk heals that. Walking a folder refreshes `scanned_at` of
    ///   every row in it, so the oldest `scanned_at` of the addon is when the last
    ///   full walk finished at the latest. That needs no extra state or migration.
    async fn load(ctx: &AppContext, addon_id: Uuid) -> Result<Option<Self>> {
        let raw: Vec<(Uuid, String, String, String, Option<String>)> = sqlx::query_as(
            "SELECT id, path, scanned_at, media_kind, imdb_id FROM opendal_files \
             WHERE addon_id = ?",
        )
        .bind(addon_id)
        .fetch_all(&ctx.db)
        .await?;
        let mut rows: Vec<IndexedRow> = Vec::with_capacity(raw.len());
        let mut oldest: Option<chrono::NaiveDateTime> = None;
        for (id, path, scanned_at, media_kind, imdb_id) in raw {
            if path.starts_with("http://") || path.starts_with("https://") {
                return Ok(None);
            }
            let Ok(at) = chrono::NaiveDateTime::parse_from_str(
                &scanned_at,
                "%Y-%m-%d %H:%M:%S%.f",
            ) else {
                return Ok(None);
            };
            oldest = Some(oldest.map_or(at, |o| o.min(at)));
            // Files that cannot be resolved are normally not stored at all, so a
            // row with no id is the only trace of a failed resolution there can be.
            let unresolved = media_kind != "track"
                && imdb_id
                    .as_deref()
                    .is_none_or(str::is_empty);
            rows.push(IndexedRow {
                id,
                path,
                scanned_at: at,
                unresolved,
            });
        }
        if oldest.is_some_and(|o| o < Utc::now().naive_utc() - FULL_WALK_INTERVAL) {
            return Ok(None);
        }
        rows.sort_unstable_by(|a, b| {
            a.path
                .cmp(&b.path)
        });
        Ok(Some(Self { rows }))
    }

    /// Ids of the folder's rows when the folder can be skipped: it already has rows,
    /// none of them failed to resolve, and its mtime is known and not newer than the
    /// newest of them.
    fn unchanged_rows(
        &self,
        dir_path: &str,
        modified: Option<chrono::DateTime<Utc>>,
    ) -> Option<Vec<Uuid>> {
        let modified = modified?.naive_utc();
        let start = self
            .rows
            .partition_point(|r| {
                r.path
                    .as_str()
                    < dir_path
            });
        let mut newest: Option<chrono::NaiveDateTime> = None;
        let mut ids = Vec::new();
        for row in self.rows[start..]
            .iter()
            .take_while(|r| {
                r.path
                    .starts_with(dir_path)
            })
        {
            if row.unresolved {
                return None;
            }
            ids.push(row.id);
            newest = Some(newest.map_or(row.scanned_at, |n| n.max(row.scanned_at)));
        }
        (modified <= newest? - INCREMENTAL_SLACK).then_some(ids)
    }
}

type EntryStream = std::pin::Pin<
    Box<dyn futures::Stream<Item = opendal::Result<opendal::Entry>> + Send>,
>;

/// Opens one configured scan path. A WebDAV path that points at a single file yields
/// just that entry; anything else is walked as a directory from its own root. With an
/// index, the directory is listed one level deep and only folders that are new or
/// changed are walked; the rows of skipped folders are added to `seen_ids` so they
/// are not pruned.
async fn open_scan_source(
    operator: &opendal::Operator,
    list_from: &str,
    is_local: bool,
    index: Option<&IncrementalIndex>,
    seen_ids: &mut Vec<Uuid>,
) -> Result<(Option<ScanEntry>, Option<EntryStream>)> {
    if is_local {
        let lister = operator
            .lister_with(list_from)
            .recursive(true)
            .await?;
        return Ok((None, Some(Box::pin(lister))));
    }
    let dir = webdav_dir_path(list_from);
    if dir != "/"
        && let Ok(meta) = operator
            .stat(dir.trim_end_matches('/'))
            .await
        && meta.mode() == EntryMode::FILE
    {
        let rel = dir
            .trim_matches('/')
            .to_string();
        return Ok((
            Some(ScanEntry {
                path: rel,
                metadata: meta,
            }),
            None,
        ));
    }
    let Some(index) = index else {
        let lister = operator
            .lister_with(&dir)
            .recursive(true)
            .await?;
        return Ok((None, Some(Box::pin(lister))));
    };

    let mut top = operator
        .lister_with(&dir)
        .await?;
    let mut direct = Vec::new();
    let mut walks = Vec::new();
    let mut skipped = 0usize;
    while let Some(entry) = top
        .try_next()
        .await?
    {
        if entry
            .metadata()
            .mode()
            != EntryMode::DIR
        {
            direct.push(entry);
            continue;
        }
        match index.unchanged_rows(
            entry.path(),
            entry
                .metadata()
                .last_modified(),
        ) {
            Some(ids) => {
                skipped += 1;
                seen_ids.extend(ids);
            }
            None => {
                walks.push(
                    operator
                        .lister_with(entry.path())
                        .recursive(true)
                        .await?,
                );
            }
        }
    }
    debug!(
        path = list_from,
        skipped,
        walked = walks.len(),
        "opendal: incremental scan"
    );
    use futures::StreamExt as _;
    Ok((
        None,
        Some(Box::pin(
            futures::stream::iter(
                direct
                    .into_iter()
                    .map(Ok),
            )
            .chain(futures::stream::iter(walks).flatten()),
        )),
    ))
}

async fn next_scan_entry(
    single: &mut Option<ScanEntry>,
    lister: &mut Option<EntryStream>,
) -> Result<Option<ScanEntry>> {
    if let Some(entry) = single.take() {
        return Ok(Some(entry));
    }
    match lister {
        Some(l) => Ok(l
            .try_next()
            .await?
            .map(ScanEntry::from)),
        None => Ok(None),
    }
}

fn build_webdav_operator(cfg: &serde_json::Value) -> Result<opendal::Operator> {
    let endpoint = cfg["endpoint"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| anyhow::anyhow!("opendal-webdav: endpoint required"))?;
    let mut builder = opendal::services::Webdav::default().endpoint(endpoint);
    if let Some(u) = cfg["username"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        builder = builder.username(u);
    }
    if let Some(p) = cfg["password"]
        .as_str()
        .filter(|s| !s.is_empty())
    {
        builder = builder.password(p);
    }
    Ok(opendal::Operator::new(builder)?.finish())
}

/// What this scan already learned about a path's tmdb/tvdb id, per kind. A
/// show's folder id sits on every file under it, so without this a miss (say
/// a series id sent to the movie endpoint) is asked again for every file.
#[derive(Default)]
struct PathIdMemo(
    std::collections::HashMap<(bool, Option<i64>, Option<i64>), Option<String>>,
);

async fn resolve_path_ids(
    memo: &mut PathIdMemo,
    ids: &db::ExternalIds,
    is_tv: bool,
    client: &sdks::RestClient<sdks::BearerAuth>,
) -> Option<String> {
    if let Some(imdb) = &ids.imdb {
        return Some(
            imdb.clone()
                .into(),
        );
    }
    let key = (is_tv, ids.tmdb, ids.tvdb);
    if let Some(known) = memo
        .0
        .get(&key)
    {
        return known.clone();
    }
    let resolved: Option<String> =
        MediaResolveService::resolve_imdb_from_ids(ids, is_tv, client)
            .await
            .map(Into::into);
    memo.0
        .insert(key, resolved.clone());
    resolved
}

async fn fetch_existing_imdb(
    ctx: &AppContext,
    addon_id: Uuid,
    path: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar(
        "SELECT imdb_id FROM opendal_files WHERE addon_id = ? AND path = ?",
    )
    .bind(addon_id)
    .bind(path)
    .fetch_optional(&ctx.db)
    .await?
    .flatten())
}

async fn prune_stale_paths(
    ctx: &AppContext,
    addon_id: Uuid,
    seen: &[Uuid],
) -> Result<usize> {
    if seen.is_empty() {
        let result = sqlx::query("DELETE FROM opendal_files WHERE addon_id = ?")
            .bind(addon_id)
            .execute(&ctx.db)
            .await?;
        return Ok(result.rows_affected() as usize);
    }

    let mut tx = ctx
        .db
        .begin()
        .await?;
    sqlx::query(
        "CREATE TEMPORARY TABLE IF NOT EXISTS _opendal_seen (id BLOB NOT NULL PRIMARY KEY)",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query("DELETE FROM _opendal_seen")
        .execute(&mut *tx)
        .await?;

    for chunk in seen.chunks(500) {
        let mut qb =
            sqlx::QueryBuilder::new("INSERT OR IGNORE INTO _opendal_seen (id) ");
        qb.push_values(chunk.iter(), |mut b, id| {
            b.push_bind(*id);
        });
        qb.build()
            .execute(&mut *tx)
            .await?;
    }

    let result = sqlx::query(
        "DELETE FROM opendal_files \
         WHERE addon_id = ? AND id NOT IN (SELECT id FROM _opendal_seen)",
    )
    .bind(addon_id)
    .execute(&mut *tx)
    .await?;

    tx.commit()
        .await?;
    Ok(result.rows_affected() as usize)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, atomic::AtomicU64};

    use chrono::Utc;
    use futures::StreamExt;
    use regex::Regex;
    use remux_sdks::stremio::ResourceType;
    use uuid::Uuid;

    use super::*;
    use crate::{
        Config,
        addons::{Addon, AddonPresetRef},
        api,
        common::{self, ProgressReporter},
        db,
        integration_test::{new_test_server, new_test_server_with_config},
        sdks,
        stream::StreamDescriptor,
    };

    #[test]
    fn webdav_dir_path_always_ends_with_slash() {
        assert_eq!(webdav_dir_path("/"), "/");
        assert_eq!(webdav_dir_path(""), "/");
        assert_eq!(
            webdav_dir_path("/Romeria (2025) [Bluray 1080p][Esp]"),
            "/Romeria (2025) [Bluray 1080p][Esp]/"
        );
        assert_eq!(webdav_dir_path("/a/b/"), "/a/b/");
    }

    fn propfind_body(entries: &[(&str, bool)]) -> String {
        let mut out =
            String::from(r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">"#);
        for (href, dir) in entries {
            let rt = if *dir {
                "<D:resourcetype><D:collection/></D:resourcetype>"
            } else {
                "<D:resourcetype/><D:getcontentlength>5</D:getcontentlength>"
            };
            out.push_str(&format!(
                "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{rt}<D:getlastmodified>Mon, 01 Jan 2024 00:00:00 GMT</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
            ));
        }
        out.push_str("</D:multistatus>");
        out
    }

    /// Minimal PROPFIND server: records every requested path.
    async fn mock_webdav(
        routes: Vec<(&'static str, String)>,
    ) -> (String, Arc<std::sync::Mutex<Vec<String>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let base = format!(
            "http://{}",
            listener
                .local_addr()
                .unwrap()
        );
        let hits = Arc::new(std::sync::Mutex::new(Vec::new()));
        let log = hits.clone();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener
                    .accept()
                    .await
                    .unwrap();
                let routes = routes.clone();
                let log = log.clone();
                tokio::spawn(async move {
                    // Read the headers up to the blank line, then any body the
                    // Content-Length announces, so the request is fully consumed.
                    let mut data: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 1024];
                    let header_end = loop {
                        if let Some(p) = data
                            .windows(4)
                            .position(|w| w == b"\r\n\r\n")
                        {
                            break p + 4;
                        }
                        match sock
                            .read(&mut chunk)
                            .await
                        {
                            Ok(0) | Err(_) => break data.len(),
                            Ok(n) => data.extend_from_slice(&chunk[..n]),
                        }
                    };
                    let head = String::from_utf8_lossy(&data[..header_end]).to_string();
                    let content_length = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| {
                                    v.trim()
                                        .parse::<usize>()
                                        .ok()
                                })
                                .flatten()
                        })
                        .unwrap_or(0);
                    while data.len() < header_end + content_length {
                        match sock
                            .read(&mut chunk)
                            .await
                        {
                            Ok(0) | Err(_) => break,
                            Ok(n) => data.extend_from_slice(&chunk[..n]),
                        }
                    }
                    let req = head;
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .to_string();
                    log.lock()
                        .unwrap()
                        .push(path.clone());
                    let (status, body) = match routes
                        .iter()
                        .find(|(p, _)| *p == path)
                    {
                        Some((_, b)) => ("207 Multi-Status", b.clone()),
                        None => ("404 Not Found", String::new()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock
                        .write_all(resp.as_bytes())
                        .await;
                });
            }
        });
        (base, hits)
    }

    // A top-level WebDAV path configured without a trailing slash used to make opendal
    // walk the whole share from the parent ("/") and prefix-filter afterwards.
    #[tokio::test]
    async fn webdav_scan_path_does_not_walk_the_whole_share() {
        let (base, hits) = mock_webdav(vec![
            (
                "/",
                propfind_body(&[
                    ("/", true),
                    ("/Movie%20A/", true),
                    ("/Movie%20B/", true),
                ]),
            ),
            (
                "/Movie%20A/",
                propfind_body(&[("/Movie%20A/", true), ("/Movie%20A/a.mkv", false)]),
            ),
            ("/c.mkv", propfind_body(&[("/c.mkv", false)])),
        ])
        .await;
        let op = opendal::Operator::new(
            opendal::services::Webdav::default().endpoint(&base),
        )
        .unwrap()
        .finish();

        let (mut single, mut lister) =
            open_scan_source(&op, "/Movie A", false, None, &mut Vec::new())
                .await
                .unwrap();
        let mut seen = Vec::new();
        while let Some(e) = next_scan_entry(&mut single, &mut lister)
            .await
            .unwrap()
        {
            if e.metadata()
                .mode()
                == EntryMode::FILE
            {
                seen.push(
                    e.path()
                        .to_string(),
                );
            }
        }
        assert_eq!(seen, vec!["Movie A/a.mkv"]);
        assert!(
            hits.lock()
                .unwrap()
                .iter()
                .all(|p| p.starts_with("/Movie%20A")),
            "crawled outside the configured path: {:?}",
            hits.lock()
                .unwrap()
        );

        // A path that is itself a file is indexed as that one file.
        let (mut single, mut lister) =
            open_scan_source(&op, "/c.mkv", false, None, &mut Vec::new())
                .await
                .unwrap();
        let first = next_scan_entry(&mut single, &mut lister)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(first.path(), "c.mkv");
        assert_eq!(first.name(), "c.mkv");
        assert!(
            next_scan_entry(&mut single, &mut lister)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !hits
                .lock()
                .unwrap()
                .contains(&"/".to_string())
        );
    }

    fn tmdb_test_client(base_url: &str) -> Option<sdks::RestClient<sdks::BearerAuth>> {
        Some(
            sdks::RestClient::new(base_url)
                .unwrap()
                .with_auth(sdks::BearerAuth {
                    token: String::new(),
                }),
        )
    }

    fn mock_tv_series(server: &httpmock::MockServer, tmdb_id: i64, imdb_id: &str) {
        let imdb = imdb_id.to_string();
        server.mock(|when, then| {
            when.path(format!("/tv/{tmdb_id}"));
            then.status(200)
                .json_body(serde_json::json!({
                    "id": tmdb_id,
                    "external_ids": { "imdb_id": imdb }
                }));
        });
    }

    fn register_all_shows(server: &httpmock::MockServer) {
        mock_tv_series(server, 157842, "tt21249100");
        mock_tv_series(server, 30984, "tt0434665");
        mock_tv_series(server, 43270, "tt1890725");
    }

    async fn test_server_with_tmdb(
        tmdb: &httpmock::MockServer,
    ) -> (crate::AppContext, crate::integration_test::TestGuard) {
        let (_, guard) = new_test_server_with_config(Config {
            database_url: Some("sqlite::memory:".into()),
            torrent_http_port: None,
            disable_dht: true,
            tmdb_base_url: tmdb.base_url(),
            ..Default::default()
        })
        .await
        .unwrap();
        let ctx = guard
            .0
            .clone();
        (ctx, guard)
    }

    fn noop_progress() -> ProgressReporter {
        ProgressReporter::new(Arc::new(AtomicU64::new(0)))
    }

    async fn make_local_addon(
        ctx: &AppContext,
        dir: &std::path::Path,
        media_kind: &str,
    ) -> (OpendalAddon, Addon) {
        let addon_id = Uuid::new_v4();
        let root = dir
            .to_str()
            .unwrap()
            .to_string();
        let now = Utc::now().naive_utc();

        let db_addon = Addon {
            id: addon_id,
            name: "test-local".to_string(),
            preset: AddonPresetRef {
                kind: "opendal-local".to_string(),
                config: serde_json::json!({
                    "media_kind": media_kind,
                    "paths": [root],
                })
                .into(),
            },
            resources: vec![ResourceType::Stream, ResourceType::Catalog],
            types: vec![],
            enabled: true,
            priority: 0,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            subtitle_extraction: false,
            probe_on_scan: false,
            service_filter: vec![],
            created_at: now,
            updated_at: now,
        };
        db_addon
            .insert(&ctx.db)
            .await
            .unwrap();

        let operator =
            opendal::Operator::new(opendal::services::Fs::default().root(&root))
                .unwrap()
                .finish();
        let addon_kind = OpendalAddon {
            addon_id,
            operator: Arc::new(operator),
            root,
            backend: "local".to_string(),
            media_kind: media_kind.to_string(),
        };

        (addon_kind, db_addon)
    }

    fn write_files(dir: &std::path::Path, files: &[(&str, &[u8])]) {
        for (rel, content) in files {
            let full = dir.join(rel);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, content).unwrap();
        }
    }

    // -----------------------------------------------------------------------
    // E2E: movies — index multiple files with varied naming and verify
    // each is scanned and streams correctly.
    // -----------------------------------------------------------------------

    struct MovieFixture {
        /// Path relative to tempdir root (directories are created automatically).
        rel_path: &'static str,
        expected_imdb: &'static str,
    }

    #[tokio::test]
    async fn opendal_local_movie_index_and_stream() {
        // Each fixture exercises a different naming convention.
        // The [imdbid-...] tag may appear anywhere in the path — file name,
        // parent folder, or grandparent folder.
        let fixtures: &[MovieFixture] = &[
            // IMDB tag embedded in the file name itself
            MovieFixture {
                rel_path: "[imdbid-tt0133093] The Matrix (1999).mkv",
                expected_imdb: "tt0133093",
            },
            // IMDB tag in a parent folder; filename uses dot-separated title + year
            MovieFixture {
                rel_path: "[imdbid-tt0816692] Interstellar (2014)/Interstellar.2014.1080p.BluRay.x264.mkv",
                expected_imdb: "tt0816692",
            },
            // IMDB tag appended at the end of the file name (before extension)
            MovieFixture {
                rel_path: "The.Wolf.of.Wall.Street.2013 [imdbid-tt0993846].mp4",
                expected_imdb: "tt0993846",
            },
            // .avi extension
            MovieFixture {
                rel_path: "[imdbid-tt1375666] Inception (2010)/Inception.2010.BluRay.avi",
                expected_imdb: "tt1375666",
            },
            // .mov extension
            MovieFixture {
                rel_path: "A Beautiful Mind [imdbid-tt0268978].mov",
                expected_imdb: "tt0268978",
            },
            // Deeply nested folder, IMDB in grandparent
            MovieFixture {
                rel_path: "[imdbid-tt0109830] Forrest Gump (1994)/1080p/Forrest.Gump.1994.mkv",
                expected_imdb: "tt0109830",
            },
        ];

        let dir = tempfile::tempdir().unwrap();
        for f in fixtures {
            let full = dir
                .path()
                .join(f.rel_path);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake").unwrap();
        }

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        for f in fixtures {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'movie' AND imdb_id = ?",
            )
            .bind(db_addon.id)
            .bind(f.expected_imdb)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
            assert_eq!(
                count, 1,
                "{}: expected 1 row for imdb {}",
                f.rel_path, f.expected_imdb
            );

            let stub = db::Media {
                id: common::get_stable_uuid(format!("movie:{}", f.expected_imdb)),
                kind: db::MediaKind::Movie,
                external_ids: db::ExternalIds {
                    imdb: db::NonEmptyString::try_new(
                        f.expected_imdb
                            .to_string(),
                    )
                    .ok(),
                    ..Default::default()
                },
                ..Default::default()
            };
            let streams = addon
                .get_streams(&stub, ctx, None)
                .await
                .unwrap();
            assert!(
                !streams.is_empty(),
                "{}: get_streams returned nothing for imdb {}",
                f.rel_path,
                f.expected_imdb
            );
            for s in &streams {
                assert!(
                    matches!(s.descriptor, StreamDescriptor::Local(_)),
                    "{}: expected Local descriptor",
                    f.rel_path
                );
            }
        }
    }

    // -----------------------------------------------------------------------
    // E2E: episodes — index multiple files with varied naming across two
    // series, verify scan results, catalog structure, and full tree from
    // get_children (Series → Seasons → Episodes).
    // -----------------------------------------------------------------------

    struct EpisodeFixture {
        rel_path: &'static str,
        expected_imdb: &'static str,
        expected_season: i64,
        expected_episode: i64,
    }

    #[tokio::test]
    async fn opendal_local_episode_index_has_seasons() {
        let fixtures: &[EpisodeFixture] = &[
            // --- Breaking Bad (tt0903747) ---
            // Standard SxxExx with quality suffix
            EpisodeFixture {
                rel_path: "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01.720p.BluRay.mkv",
                expected_imdb: "tt0903747",
                expected_season: 1,
                expected_episode: 1,
            },
            // Lowercase sXXeXX
            EpisodeFixture {
                rel_path: "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.s01e02.mkv",
                expected_imdb: "tt0903747",
                expected_season: 1,
                expected_episode: 2,
            },
            // NxNN alternative format
            EpisodeFixture {
                rel_path: "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.1x03.1080p.mkv",
                expected_imdb: "tt0903747",
                expected_season: 1,
                expected_episode: 3,
            },
            // Second season, .avi extension
            EpisodeFixture {
                rel_path: "[imdbid-tt0903747] Breaking Bad/Season 02/Breaking.Bad.S02E01.WEB-DL.mkv",
                expected_imdb: "tt0903747",
                expected_season: 2,
                expected_episode: 1,
            },
            // Underscore separators instead of dots
            EpisodeFixture {
                rel_path: "[imdbid-tt0903747] Breaking Bad/Season 02/Breaking_Bad_S02E02.avi",
                expected_imdb: "tt0903747",
                expected_season: 2,
                expected_episode: 2,
            },
            // --- Game of Thrones (tt0944947) — no Season sub-folders ---
            // Standard SxxExx
            EpisodeFixture {
                rel_path: "[imdbid-tt0944947] Game of Thrones/Game.of.Thrones.S01E01.mkv",
                expected_imdb: "tt0944947",
                expected_season: 1,
                expected_episode: 1,
            },
            // Mixed year + episode code
            EpisodeFixture {
                rel_path: "[imdbid-tt0944947] Game of Thrones/Game.of.Thrones.2011.S01E02.mkv",
                expected_imdb: "tt0944947",
                expected_season: 1,
                expected_episode: 2,
            },
            // [imdb-tt...] tag (no "id" suffix), title with year + episode title suffix
            EpisodeFixture {
                rel_path: "Derry Girls (2018) [imdb-tt7120662]/Season 01 [imdb-tt7120662]/Derry Girls (2018) - S01E01 - Episode 1 [WEBDL-1080p][EAC3 2.0][h265]-MZABI [imdb-tt7120662].mkv",
                expected_imdb: "tt7120662",
                expected_season: 1,
                expected_episode: 1,
            },
            // --- Black Summoner (2022) [tvdbid-416588] ---
            EpisodeFixture {
                rel_path: "[imdbid-tt21249100] Black Summoner (2022)/Season 01/Black.Summoner.S01E01.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 1,
            },
            EpisodeFixture {
                rel_path: "[imdbid-tt21249100] Black Summoner (2022)/Season 01/Black.Summoner.S01E02.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 2,
            },
            // --- Bleach (2004) [tvdbid-74796] ---
            EpisodeFixture {
                rel_path: "[imdbid-tt0434665] Bleach (2004)/Season 01/Bleach.S01E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 1,
            },
            EpisodeFixture {
                rel_path: "[imdbid-tt0434665] Bleach (2004)/Season 01/Bleach.S01E02.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 2,
            },
            EpisodeFixture {
                rel_path: "[imdbid-tt0434665] Bleach (2004)/Season 02/Bleach.S02E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 2,
                expected_episode: 1,
            },
            // --- Blood-C (2011) [tvdbid-249864] ---
            EpisodeFixture {
                rel_path: "[imdbid-tt1890725] Blood-C (2011)/Season 01/Blood-C.S01E01.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 1,
            },
            EpisodeFixture {
                rel_path: "[imdbid-tt1890725] Blood-C (2011)/Season 01/Blood-C.S01E02.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 2,
            },
        ];

        let dir = tempfile::tempdir().unwrap();
        for f in fixtures {
            let full = dir
                .path()
                .join(f.rel_path);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // Every fixture must produce exactly one opendal_files row.
        for f in fixtures {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'episode' \
                   AND imdb_id = ? AND season = ? AND episode = ?",
            )
            .bind(db_addon.id)
            .bind(f.expected_imdb)
            .bind(f.expected_season)
            .bind(f.expected_episode)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
            assert_eq!(
                count, 1,
                "{}: expected row with imdb={} s={} e={}",
                f.rel_path, f.expected_imdb, f.expected_season, f.expected_episode
            );
        }

        // Catalog must yield one Series per distinct IMDB.
        let stream = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap();
        let mut series_items: Vec<db::Media> = stream
            .collect()
            .await;
        series_items.sort_by(|a, b| {
            a.external_ids
                .imdb
                .cmp(
                    &b.external_ids
                        .imdb,
                )
        });
        assert_eq!(series_items.len(), 6, "catalog should contain six Series");
        assert!(
            series_items
                .iter()
                .all(|s| s.kind == db::MediaKind::Series)
        );

        // For each series verify full tree via get_children.
        for series in &series_items {
            let imdb = series
                .external_ids
                .imdb
                .as_deref()
                .unwrap();

            let seasons = addon
                .get_children(series, ctx)
                .await
                .unwrap()
                .unwrap_or_else(|| {
                    panic!("{imdb}: get_children(Series) returned None")
                });
            assert!(!seasons.is_empty(), "{imdb}: expected at least one Season");
            assert!(
                seasons
                    .iter()
                    .all(|s| s.kind == db::MediaKind::Season),
                "{imdb}: all children of a Series must be Seasons"
            );
            assert!(
                seasons
                    .iter()
                    .all(|s| s.kind == db::MediaKind::Season),
                "{imdb}: all children of a Series must be Seasons"
            );

            let expected_season_nums: Vec<i64> = {
                let mut v: Vec<i64> = fixtures
                    .iter()
                    .filter(|f| f.expected_imdb == imdb)
                    .map(|f| f.expected_season)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                v.sort();
                v
            };
            let mut got_season_nums: Vec<i64> = seasons
                .iter()
                .filter_map(|s| s.idx)
                .collect();
            got_season_nums.sort();
            assert_eq!(
                got_season_nums, expected_season_nums,
                "{imdb}: wrong set of season numbers"
            );

            for season in &seasons {
                let season_num = season
                    .idx
                    .unwrap();
                let episodes = addon
                    .get_children(season, ctx)
                    .await
                    .unwrap()
                    .unwrap_or_else(|| {
                        panic!(
                            "{imdb} s{season_num}: get_children(Season) returned None"
                        )
                    });
                assert!(
                    !episodes.is_empty(),
                    "{imdb} s{season_num}: expected episodes"
                );
                assert!(
                    episodes
                        .iter()
                        .all(|e| e.kind == db::MediaKind::Episode),
                    "{imdb} s{season_num}: all children of a Season must be Episodes"
                );
                assert!(
                    episodes
                        .iter()
                        .all(|e| e.kind == db::MediaKind::Episode),
                    "{imdb} s{season_num}: all children of a Season must be Episodes"
                );

                let expected_ep_nums: Vec<i64> = {
                    let mut v: Vec<i64> = fixtures
                        .iter()
                        .filter(|f| {
                            f.expected_imdb == imdb && f.expected_season == season_num
                        })
                        .map(|f| f.expected_episode)
                        .collect();
                    v.sort();
                    v
                };
                let mut got_ep_nums: Vec<i64> = episodes
                    .iter()
                    .filter_map(|e| e.idx)
                    .collect();
                got_ep_nums.sort();
                assert_eq!(
                    got_ep_nums, expected_ep_nums,
                    "{imdb} s{season_num}: wrong set of episode numbers"
                );

                // Every episode must carry a Local stream descriptor.
                for ep in &episodes {
                    let info = ep
                        .stream_info
                        .as_ref()
                        .unwrap_or_else(|| {
                            panic!("{imdb} s{season_num} e{:?}: no stream_info", ep.idx)
                        });
                    assert!(
                        matches!(info.descriptor, StreamDescriptor::Local(_)),
                        "{imdb} s{season_num} e{:?}: expected Local stream",
                        ep.idx
                    );
                }
            }
        }
    }

    // ---------------------------------------------------------------------------
    // Episode stream_supports + get_streams use the grandparent's IMDB (not the episode's own).
    // The existing tree-walk test only exercises get_children; this test covers
    // the separate path used when the server resolves streams for a known media row.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_episode_stream_supports_and_get_streams() {
        let fixtures: &[(&str, &str, i64, i64)] = &[
            // (rel_path, series_imdb_for_filename, season, episode)
            (
                "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01.720p.mkv",
                "tt0903747",
                1,
                1,
            ),
            (
                "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E02.mkv",
                "tt0903747",
                1,
                2,
            ),
        ];

        let dir = tempfile::tempdir().unwrap();
        for (rel, _, _, _) in fixtures {
            let full = dir
                .path()
                .join(rel);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // Walk Series → Season → Episodes to get real Episode rows with grandparent set.
        let stream = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap();
        let series_items: Vec<db::Media> = stream
            .collect()
            .await;
        assert_eq!(series_items.len(), 1);

        let seasons = addon
            .get_children(&series_items[0], ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(seasons.len(), 1);

        let episodes = addon
            .get_children(&seasons[0], ctx)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(episodes.len(), 2);

        for ep in &episodes {
            // stream_supports must return true — episodes returned by get_children have
            // grandparent set (via DB lookup), so supports() correctly returns true.
            assert!(
                StreamAddon::supports(&addon, ep),
                "stream_supports must be true for episode with grandparent imdb set (e{:?})",
                ep.idx
            );

            let streams = addon
                .get_streams(ep, ctx, None)
                .await
                .unwrap();
            assert!(
                !streams.is_empty(),
                "get_streams must return at least one stream for e{:?}",
                ep.idx
            );
            for s in &streams {
                assert!(
                    matches!(s.descriptor, StreamDescriptor::Local(_)),
                    "expected Local stream descriptor for e{:?}",
                    ep.idx
                );
            }
        }

        // Negative: an Episode row without a grandparent (no series IMDB accessible)
        // must return false — the opendal_files table stores series IMDB, not episode IMDB.
        let ep_without_grandparent = db::Media {
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0903747").ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !StreamAddon::supports(&addon, &ep_without_grandparent),
            "stream_supports must be false for episode without grandparent"
        );
    }

    // A local "episode" source is a Series library, so catalogs_for_kinds must
    // admit its catalog when Series is requested (as RefreshLibraryTask does), but
    // not for an unrelated kind.
    #[tokio::test]
    async fn episode_catalog_admitted_when_series_requested() {
        let dir = tempfile::tempdir().unwrap();
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (_, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        ctx.addons
            .reload(&ctx.db, &ctx.config)
            .await
            .unwrap();

        for (requested, should_admit) in
            [(db::MediaKind::Series, true), (db::MediaKind::Movie, false)]
        {
            let admitted = ctx
                .addons
                .catalogs_for_kinds(ctx, &[requested.clone()])
                .await
                .into_iter()
                .find(|(rt, _)| {
                    rt.row
                        .id
                        == db_addon.id
                })
                .is_some_and(|(_, cats)| !cats.is_empty());
            assert_eq!(
                admitted, should_admit,
                "episode catalog admission for {requested:?} should be {should_admit}"
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Files inside special-feature subdirs (trailers/, extras/, etc.) must be
    // skipped — they are not episodes or movies.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_skips_special_feature_subdirs() {
        // A valid episode alongside trailer and extras files for the same show.
        let valid =
            "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01.720p.mkv";
        let skip_cases = &[
            // directory-based skips
            "[imdbid-tt0903747] Breaking Bad/trailers/Final Trailer.mkv",
            "[imdbid-tt0903747] Breaking Bad/Trailers/Final Trailer 2.mkv", // capital T
            "[imdbid-tt0903747] Breaking Bad/extras/Gag Reel.mkv",
            "[imdbid-tt0903747] Breaking Bad/behind the scenes/Making Of.mkv",
            "[imdbid-tt0903747] Breaking Bad/featurettes/Chemistry.mkv",
            "[imdbid-tt0903747] Breaking Bad/interviews/Bryan Cranston.mkv",
            "[imdbid-tt0903747] Breaking Bad/deleted scenes/Cut S01E01.mkv",
            "[imdbid-tt0903747] Breaking Bad/backdrops/Backdrop.mkv",
            "[imdbid-tt0903747] Breaking Bad/clips/Clip.mkv",
            "[imdbid-tt0903747] Breaking Bad/other/Other.mkv",
            // filename exact-stem skips
            "[imdbid-tt0903747] Breaking Bad/Season 01/trailer.mkv",
            "[imdbid-tt0903747] Breaking Bad/Season 01/sample.mkv",
            // filename suffix skips
            "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01-trailer.mkv",
            "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01-sample.mkv",
            "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01-featurette.mkv",
            "[imdbid-tt0903747] Breaking Bad/Season 01/Breaking.Bad.S01E01-deleted.mkv",
        ];

        let dir = tempfile::tempdir().unwrap();
        for rel in std::iter::once(valid).chain(
            skip_cases
                .iter()
                .copied(),
        ) {
            let full = dir
                .path()
                .join(rel);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake").unwrap();
        }

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // Only the valid episode file must have a row.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM opendal_files WHERE addon_id = ?")
                .bind(db_addon.id)
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "expected exactly 1 row (the valid episode); trailers/extras must be skipped"
        );
    }

    // ---------------------------------------------------------------------------
    // Non-video files (e.g. thumbnails) must be silently skipped — no error,
    // no opendal_files row.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_local_skips_thumbnail_jpg() {
        let rel_path = "3 Body Problem (2024) [imdb-tt13016388]/3 Body Problem (2024) - S01E01 - Countdown [HDTV-2160p][EAC3 5.1][h265] [imdb-tt13016388]-thumb.jpg";

        let dir = tempfile::tempdir().unwrap();
        let full = dir
            .path()
            .join(rel_path);
        std::fs::create_dir_all(
            full.parent()
                .unwrap(),
        )
        .unwrap();
        std::fs::write(&full, b"fake thumb").unwrap();

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM opendal_files WHERE addon_id = ?")
                .bind(db_addon.id)
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(
            count, 0,
            "thumbnail jpg must not produce any opendal_files row"
        );
    }

    // ---------------------------------------------------------------------------
    // Movie filename parsing (title + year via hunch)
    // ---------------------------------------------------------------------------

    struct MovieCase {
        stem: &'static str,
        title: &'static str,
        year: Option<i32>,
    }

    #[test]
    fn movie_filename_parsing() {
        let cases = [
            MovieCase {
                stem: "The.Matrix.1999.1080p.BluRay.x264",
                title: "The Matrix",
                year: Some(1999),
            },
            MovieCase {
                stem: "3.Days.to.Kill.2014.720p.BluRay.x264-YIFY",
                title: "3 Days to Kill",
                year: Some(2014),
            },
            MovieCase {
                stem: "Brave (2006)",
                title: "Brave",
                year: Some(2006),
            },
            MovieCase {
                stem: "The Wolf of Wall Street (2013)",
                title: "The Wolf of Wall Street",
                year: Some(2013),
            },
            MovieCase {
                stem: "curse.of.chucky.2013.stv.unrated.multi.1080p",
                title: "curse of chucky",
                year: Some(2013),
            },
        ];

        for c in &cases {
            let parsed = hunch::hunch(c.stem);
            let title = movie_title_from_stem(
                c.stem,
                parsed.title(),
                parsed
                    .year()
                    .map(|y| y as i64),
            );
            assert_eq!(title, c.title, "title mismatch for {:?}", c.stem);
            assert_eq!(parsed.year(), c.year, "year mismatch for {:?}", c.stem);
        }
    }

    #[test]
    fn trash_guides_movie_path_preserves_part_title() {
        let path = "/media/movies/Dune Part Two (2024)/Dune Part Two (2024).mkv";
        let stem = std::path::Path::new(path)
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap();
        let parsed = hunch::hunch(stem);

        assert_eq!(
            movie_title_from_stem(
                stem,
                parsed.title(),
                parsed
                    .year()
                    .map(|y| y as i64),
            ),
            "Dune Part Two"
        );
        assert_eq!(parsed.year(), Some(2024));
        assert_eq!(parsed.part(), Some(2));
    }

    // ---------------------------------------------------------------------------
    // Episode filename parsing (title + season + episode via hunch)
    // ---------------------------------------------------------------------------

    struct EpisodeCase {
        stem: &'static str,
        title: &'static str,
        season: Option<i32>,
        episode: Option<i32>,
    }

    #[test]
    fn episode_filename_parsing() {
        let cases = [
            EpisodeCase {
                stem: "Breaking.Bad.S01E05.720p.BluRay",
                title: "Breaking Bad",
                season: Some(1),
                episode: Some(5),
            },
            EpisodeCase {
                stem: "The.Walking.Dead.4x01.720p",
                title: "The Walking Dead",
                season: Some(4),
                episode: Some(1),
            },
            EpisodeCase {
                stem: "anything_s01e02",
                title: "anything",
                season: Some(1),
                episode: Some(2),
            },
            EpisodeCase {
                stem: "Foo.2019.S04E03",
                title: "Foo",
                season: Some(4),
                episode: Some(3),
            },
            // Space-dash-space separators; episode title whose first token looks like a
            // timecode — reported as not producing streams.
            EpisodeCase {
                stem: "Chernobyl - S01E01 - 1 23 45",
                title: "Chernobyl",
                season: Some(1),
                episode: Some(1),
            },
            EpisodeCase {
                stem: "Chernobyl - S01E02 - Please Remain Calm",
                title: "Chernobyl",
                season: Some(1),
                episode: Some(2),
            },
        ];

        for c in &cases {
            let parsed = hunch::hunch(c.stem);
            assert_eq!(
                parsed
                    .title()
                    .unwrap_or(""),
                c.title,
                "title mismatch for {:?}",
                c.stem
            );
            assert_eq!(
                parsed.season(),
                c.season,
                "season mismatch for {:?}",
                c.stem
            );
            assert_eq!(
                parsed.episode(),
                c.episode,
                "episode mismatch for {:?}",
                c.stem
            );
        }
    }

    // ---------------------------------------------------------------------------
    // Track leading-number stripping (track_num_re)
    // ---------------------------------------------------------------------------

    struct TrackCase {
        stem: &'static str,
        track_number: Option<i64>,
        remainder: &'static str,
    }

    #[test]
    fn track_number_stripping() {
        let re = Regex::new(r"^(\d{1,3})[.\s\-_\[\]]+").unwrap();

        let cases = [
            TrackCase {
                stem: "01. Artist - Song",
                track_number: Some(1),
                remainder: "Artist - Song",
            },
            TrackCase {
                stem: "03 - Another Song",
                track_number: Some(3),
                remainder: "Another Song",
            },
            TrackCase {
                stem: "123_Track Name",
                track_number: Some(123),
                remainder: "Track Name",
            },
            TrackCase {
                stem: "Song Without Number",
                track_number: None,
                remainder: "Song Without Number",
            },
        ];

        for c in &cases {
            let track_number = re
                .captures(c.stem)
                .and_then(|cap| cap.get(1))
                .and_then(|m| {
                    m.as_str()
                        .parse::<i64>()
                        .ok()
                });

            let remainder = if track_number.is_some() {
                re.replace(c.stem, "")
                    .into_owned()
            } else {
                c.stem
                    .to_string()
            };

            assert_eq!(
                track_number, c.track_number,
                "track_number mismatch for {:?}",
                c.stem
            );
            assert_eq!(
                remainder.trim(),
                c.remainder,
                "remainder mismatch for {:?}",
                c.stem
            );
        }
    }

    // -----------------------------------------------------------------------
    // Subtitle stem splitter unit tests
    // -----------------------------------------------------------------------

    #[test]
    fn opendal_subtitle_suffix_parser() {
        struct Case {
            stem: &'static str,
            base: &'static str,
            lang: Option<&'static str>,
            is_forced: bool,
            is_hi: bool,
        }

        let cases = &[
            Case {
                stem: "The.Matrix.1999.en",
                base: "The.Matrix.1999",
                lang: Some("en"),
                is_forced: false,
                is_hi: false,
            },
            Case {
                stem: "The.Matrix.1999.fr.forced",
                base: "The.Matrix.1999",
                lang: Some("fr"),
                is_forced: true,
                is_hi: false,
            },
            Case {
                stem: "The.Matrix.1999.de.hi",
                base: "The.Matrix.1999",
                lang: Some("de"),
                is_forced: false,
                is_hi: true,
            },
            Case {
                stem: "The.Matrix.1999",
                base: "The.Matrix.1999",
                lang: None,
                is_forced: false,
                is_hi: false,
            },
            Case {
                stem: "The.Matrix.1999.en.forced.hi",
                base: "The.Matrix.1999",
                lang: Some("en"),
                is_forced: true,
                is_hi: true,
            },
            Case {
                stem: "Breaking.Bad.S01E01.en.forced",
                base: "Breaking.Bad.S01E01",
                lang: Some("en"),
                is_forced: true,
                is_hi: false,
            },
            Case {
                stem: "Movie.sdh",
                base: "Movie",
                lang: None,
                is_forced: false,
                is_hi: true,
            },
        ];

        for c in cases {
            let (base, lang, is_forced, is_hi) = split_subtitle_stem(c.stem);
            assert_eq!(base, c.base, "base mismatch for {:?}", c.stem);
            assert_eq!(lang.as_deref(), c.lang, "lang mismatch for {:?}", c.stem);
            assert_eq!(
                is_forced, c.is_forced,
                "is_forced mismatch for {:?}",
                c.stem
            );
            assert_eq!(is_hi, c.is_hi, "is_hi mismatch for {:?}", c.stem);
        }
    }

    // -----------------------------------------------------------------------
    // E2E: subtitle scan + subtitle_fetch
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_subtitle_scan_and_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let base = "[imdbid-tt0133093] The Matrix (1999)";
        // Subtitle files only — no video required.
        std::fs::write(
            dir.path()
                .join(format!("{base}.en.srt")),
            b"subtitle",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(format!("{base}.fr.forced.vtt")),
            b"subtitle",
        )
        .unwrap();
        std::fs::write(
            dir.path()
                .join(format!("{base}.de.hi.ass")),
            b"subtitle",
        )
        .unwrap();

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // Three subtitle rows should be indexed with the correct IMDB id.
        let sub_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files \
             WHERE addon_id = ? AND media_kind = 'subtitle' AND imdb_id = 'tt0133093'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(sub_count, 3, "expected 3 subtitle rows");

        // subtitle_fetch returns SubtitleInfo with Opendal descriptors.
        let movie_media = db::Media {
            id: crate::common::get_stable_uuid("movie:tt0133093".to_string()),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0133093".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let infos = addon
            .subtitle_fetch(&movie_media, &SubtitleQuery::default(), &ctx.db)
            .await
            .unwrap();
        assert_eq!(infos.len(), 3, "subtitle_fetch should return 3 subtitles");

        for info in &infos {
            assert!(
                matches!(info.url, Some(StreamDescriptor::Opendal { .. })),
                "expected Opendal descriptor"
            );
        }

        let en = infos
            .iter()
            .find(|s| {
                s.lang
                    .as_deref()
                    == Some("en")
            })
            .expect("English subtitle not found");
        assert!(!en.is_forced);
        assert!(!en.is_hi);

        let fr = infos
            .iter()
            .find(|s| {
                s.lang
                    .as_deref()
                    == Some("fr")
            })
            .expect("French subtitle not found");
        assert!(fr.is_forced);
        assert!(!fr.is_hi);

        let de = infos
            .iter()
            .find(|s| {
                s.lang
                    .as_deref()
                    == Some("de")
            })
            .expect("German subtitle not found");
        assert!(!de.is_forced);
        assert!(de.is_hi);
    }

    // -----------------------------------------------------------------------
    // E2E: subtitle without an IMDB id is not indexed
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_subtitle_no_orphan() {
        let dir = tempfile::tempdir().unwrap();
        // No [imdbid-...] tag and no TMDB client → cannot resolve IMDB → skipped.
        std::fs::write(
            dir.path()
                .join("unresolvable.en.srt"),
            b"subtitle",
        )
        .unwrap();
        // This one has an IMDB tag and must be indexed.
        std::fs::write(
            dir.path()
                .join("[imdbid-tt0133093] The Matrix (1999).en.srt"),
            b"subtitle",
        )
        .unwrap();

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let sub_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files WHERE addon_id = ? AND media_kind = 'subtitle'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(
            sub_count, 1,
            "only the subtitle with an IMDB tag should be indexed"
        );
    }

    // -----------------------------------------------------------------------
    // E2E: stale subtitle rows are pruned on re-index
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_subtitle_stale_prune() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir
            .path()
            .join("[imdbid-tt0133093] The Matrix (1999).en.srt");
        std::fs::write(&sub, b"subtitle").unwrap();

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let sub_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files WHERE addon_id = ? AND media_kind = 'subtitle'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(sub_count, 1, "subtitle should be indexed after first scan");

        // Delete the subtitle file and re-index.
        std::fs::remove_file(&sub).unwrap();
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let sub_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files WHERE addon_id = ? AND media_kind = 'subtitle'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(sub_count, 0, "stale subtitle row should be pruned");
    }

    // -----------------------------------------------------------------------
    // resolve_imdb: title-search fallback (live TMDB) — the path taken when a
    // file has no external-id tag at all and title+year are parsed from the name.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn resolve_imdb_by_title_black_summoner() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Black Summoner");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 157842, "name": "Black Summoner"}]
                }));
        });
        mock_tv_series(&server, 157842, "tt21249100");

        let result: Option<String> = MediaResolveService::resolve_imdb_from_search(
            &tmdb_test_client(&server.base_url()).unwrap(),
            "Black Summoner",
            Some(2022),
            true,
        )
        .await
        .map(Into::into);
        assert_eq!(
            result.as_deref(),
            Some("tt21249100"),
            "Black Summoner title search"
        );
    }

    #[tokio::test]
    async fn resolve_imdb_by_title_bleach() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Bleach");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 30984, "name": "Bleach"}]
                }));
        });
        mock_tv_series(&server, 30984, "tt0434665");

        let result: Option<String> = MediaResolveService::resolve_imdb_from_search(
            &tmdb_test_client(&server.base_url()).unwrap(),
            "Bleach",
            Some(2004),
            true,
        )
        .await
        .map(Into::into);
        assert_eq!(result.as_deref(), Some("tt0434665"), "Bleach title search");
    }

    #[tokio::test]
    async fn resolve_imdb_by_title_blood_c() {
        let server = httpmock::MockServer::start();
        server.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Blood-C");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 43270, "name": "Blood-C"}]
                }));
        });
        mock_tv_series(&server, 43270, "tt1890725");

        let result: Option<String> = MediaResolveService::resolve_imdb_from_search(
            &tmdb_test_client(&server.base_url()).unwrap(),
            "Blood-C",
            Some(2011),
            true,
        )
        .await
        .map(Into::into);
        assert_eq!(result.as_deref(), Some("tt1890725"), "Blood-C title search");
    }

    // -----------------------------------------------------------------------
    // E2E: tvdbid-tagged episodes — scanner resolves tvdbid → imdbid via live
    // TMDB and stores the resolved imdb_id in opendal_files.
    // -----------------------------------------------------------------------

    struct ResolveFixture {
        rel_path: &'static str,
        expected_imdb: &'static str,
        expected_season: i64,
        expected_episode: i64,
    }

    #[tokio::test]
    async fn opendal_local_episode_tvdb_resolve() {
        let fixtures: &[ResolveFixture] = &[
            // --- Black Summoner (2022) [tvdbid-416588] → tt21249100 ---
            ResolveFixture {
                rel_path: "Black Summoner (2022) [tvdbid-416588]/Season 01/Black.Summoner.S01E01.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Black Summoner (2022) [tvdbid-416588]/Season 01/Black.Summoner.S01E02.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 2,
            },
            // --- Bleach (2004) [tvdbid-74796] → tt0434665 ---
            ResolveFixture {
                rel_path: "Bleach (2004) [tvdbid-74796]/Season 01/Bleach.S01E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Bleach (2004) [tvdbid-74796]/Season 01/Bleach.S01E02.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 2,
            },
            ResolveFixture {
                rel_path: "Bleach (2004) [tvdbid-74796]/Season 02/Bleach.S02E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 2,
                expected_episode: 1,
            },
            // --- Blood-C (2011) [tvdbid-249864] → tt1890725 ---
            ResolveFixture {
                rel_path: "Blood-C (2011) [tvdbid-249864]/Season 01/Blood-C.S01E01.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Blood-C (2011) [tvdbid-249864]/Season 01/Blood-C.S01E02.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 2,
            },
        ];

        let dir = tempfile::tempdir().unwrap();
        for f in fixtures {
            let full = dir
                .path()
                .join(f.rel_path);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let tmdb = httpmock::MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/find/416588")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 157842, "name": "Black Summoner"}],
                    "movie_results": []
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/find/74796")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 30984, "name": "Bleach"}],
                    "movie_results": []
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/find/249864")
                .query_param("external_source", "tvdb_id");
            then.status(200)
                .json_body(serde_json::json!({
                    "tv_results": [{"id": 43270, "name": "Blood-C"}],
                    "movie_results": []
                }));
        });
        register_all_shows(&tmdb);

        let (ctx, _guard) = test_server_with_tmdb(&tmdb).await;

        let (addon, db_addon) = make_local_addon(&ctx, dir.path(), "episode").await;
        addon
            .refresh_index(&ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        for f in fixtures {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'episode' \
                   AND imdb_id = ? AND season = ? AND episode = ?",
            )
            .bind(db_addon.id)
            .bind(f.expected_imdb)
            .bind(f.expected_season)
            .bind(f.expected_episode)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
            assert_eq!(
                count, 1,
                "{}: expected imdb={} s={} e={} after tvdbid→imdb resolution",
                f.rel_path, f.expected_imdb, f.expected_season, f.expected_episode
            );
        }
    }

    // A show's tmdb id sits on every file in its folder. Under a movie addon it
    // goes to the movie endpoint and 404s; that miss must be asked once per
    // scan, not once per file.
    #[tokio::test]
    async fn opendal_movie_scan_asks_a_missing_tmdb_id_once() {
        let dir = tempfile::tempdir().unwrap();
        for e in 1..=3 {
            let full = dir
                .path()
                .join(format!(
                    "Some Show (2005) [tmdbid-2710]/Season 01/Some.Show.S01E0{e}.mkv"
                ));
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let tmdb = httpmock::MockServer::start();
        let movie = tmdb.mock(|when, then| {
            when.path("/movie/2710");
            then.status(404);
        });

        let (ctx, _guard) = test_server_with_tmdb(&tmdb).await;
        let (addon, db_addon) = make_local_addon(&ctx, dir.path(), "movie").await;
        addon
            .refresh_index(&ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        assert_eq!(movie.hits(), 1);
    }

    // -----------------------------------------------------------------------
    // E2E: tmdbid-tagged episodes — scanner resolves tmdbid → imdbid via
    // SeriesEndpoint and stores the result.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_local_episode_tmdbid_resolve() {
        let fixtures: &[ResolveFixture] = &[
            // --- Black Summoner (2022) [tmdbid-157842] → tt21249100 ---
            ResolveFixture {
                rel_path: "Black Summoner (2022) [tmdbid-157842]/Season 01/Black.Summoner.S01E01.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Black Summoner (2022) [tmdbid-157842]/Season 01/Black.Summoner.S01E02.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 2,
            },
            // --- Bleach (2004) [tmdbid-30984] → tt0434665 ---
            ResolveFixture {
                rel_path: "Bleach (2004) [tmdbid-30984]/Season 01/Bleach.S01E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Bleach (2004) [tmdbid-30984]/Season 02/Bleach.S02E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 2,
                expected_episode: 1,
            },
            // --- Blood-C (2011) [tmdbid-43270] → tt1890725 ---
            ResolveFixture {
                rel_path: "Blood-C (2011) [tmdbid-43270]/Season 01/Blood-C.S01E01.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Blood-C (2011) [tmdbid-43270]/Season 01/Blood-C.S01E02.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 2,
            },
        ];

        let dir = tempfile::tempdir().unwrap();
        for f in fixtures {
            let full = dir
                .path()
                .join(f.rel_path);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let tmdb = httpmock::MockServer::start();
        register_all_shows(&tmdb);

        let (ctx, _guard) = test_server_with_tmdb(&tmdb).await;

        let (addon, db_addon) = make_local_addon(&ctx, dir.path(), "episode").await;
        addon
            .refresh_index(&ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        for f in fixtures {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'episode' \
                   AND imdb_id = ? AND season = ? AND episode = ?",
            )
            .bind(db_addon.id)
            .bind(f.expected_imdb)
            .bind(f.expected_season)
            .bind(f.expected_episode)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
            assert_eq!(
                count, 1,
                "{}: expected imdb={} s={} e={} after tmdbid→imdb resolution",
                f.rel_path, f.expected_imdb, f.expected_season, f.expected_episode
            );
        }
    }

    // -----------------------------------------------------------------------
    // E2E: no-label episodes — no external ID tag anywhere in the path; scanner
    // falls back to title+year search (resolve_imdb) to find the imdbid.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_local_episode_no_label_resolve() {
        let fixtures: &[ResolveFixture] = &[
            // --- Black Summoner — parsed title: "Black Summoner" → tt21249100 ---
            ResolveFixture {
                rel_path: "Black Summoner/Season 01/Black.Summoner.S01E01.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Black Summoner/Season 01/Black.Summoner.S01E02.mkv",
                expected_imdb: "tt21249100",
                expected_season: 1,
                expected_episode: 2,
            },
            // --- Bleach — parsed title: "Bleach" → tt0434665 ---
            ResolveFixture {
                rel_path: "Bleach/Season 01/Bleach.S01E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Bleach/Season 02/Bleach.S02E01.mkv",
                expected_imdb: "tt0434665",
                expected_season: 2,
                expected_episode: 1,
            },
            // --- Blood-C — parsed title: "Blood-C" → tt1890725 ---
            ResolveFixture {
                rel_path: "Blood-C/Season 01/Blood-C.S01E01.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 1,
            },
            ResolveFixture {
                rel_path: "Blood-C/Season 01/Blood-C.S01E02.mkv",
                expected_imdb: "tt1890725",
                expected_season: 1,
                expected_episode: 2,
            },
        ];

        let dir = tempfile::tempdir().unwrap();
        for f in fixtures {
            let full = dir
                .path()
                .join(f.rel_path);
            std::fs::create_dir_all(
                full.parent()
                    .unwrap(),
            )
            .unwrap();
            std::fs::write(&full, b"fake ep").unwrap();
        }

        let tmdb = httpmock::MockServer::start();
        tmdb.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Black Summoner");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 157842, "name": "Black Summoner"}]
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Bleach");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 30984, "name": "Bleach"}]
                }));
        });
        tmdb.mock(|when, then| {
            when.path("/search/tv")
                .query_param("query", "Blood-C");
            then.status(200)
                .json_body(serde_json::json!({
                    "results": [{"id": 43270, "name": "Blood-C"}]
                }));
        });
        register_all_shows(&tmdb);

        let (ctx, _guard) = test_server_with_tmdb(&tmdb).await;

        let (addon, db_addon) = make_local_addon(&ctx, dir.path(), "episode").await;
        addon
            .refresh_index(&ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        for f in fixtures {
            let count: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'episode' \
                   AND imdb_id = ? AND season = ? AND episode = ?",
            )
            .bind(db_addon.id)
            .bind(f.expected_imdb)
            .bind(f.expected_season)
            .bind(f.expected_episode)
            .fetch_one(&ctx.db)
            .await
            .unwrap();
            assert_eq!(
                count, 1,
                "{}: expected imdb={} s={} e={} after title-search resolution",
                f.rel_path, f.expected_imdb, f.expected_season, f.expected_episode
            );
        }
    }

    // -----------------------------------------------------------------------
    // E2E: track indexing — track_number extraction, catalog, and get_streams.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_local_track_index_and_stream() {
        let dir = tempfile::tempdir().unwrap();
        write_files(
            dir.path(),
            &[
                ("01 - First Song.mp3", b"audio"),
                ("02. Second Song.flac", b"audio"),
                ("Track Without Number.ogg", b"audio"),
            ],
        );

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "track").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // Verify track_number and title are stored correctly.
        let rows: Vec<(Option<String>, Option<i64>)> = sqlx::query_as(
            "SELECT title, track_number FROM opendal_files \
             WHERE addon_id = ? AND media_kind = 'track' ORDER BY COALESCE(track_number, 999)",
        )
        .bind(db_addon.id)
        .fetch_all(&ctx.db)
        .await
        .unwrap();

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], (Some("First Song".to_string()), Some(1)));
        assert_eq!(rows[1], (Some("Second Song".to_string()), Some(2)));
        assert_eq!(
            rows[2].1, None,
            "unnumbered track must have track_number=NULL"
        );

        // catalog_stream must return one Track per file.
        let catalog: Vec<db::Media> = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap()
            .collect()
            .await;
        assert_eq!(catalog.len(), 3);
        assert!(
            catalog
                .iter()
                .all(|m| m.kind == db::MediaKind::Track)
        );

        // get_streams must return a Local stream for each track (matched by title).
        for item in &catalog {
            let streams = addon
                .get_streams(item, ctx, None)
                .await
                .unwrap();
            assert!(
                !streams.is_empty(),
                "get_streams empty for track {:?}",
                item.title
            );
            assert!(
                streams
                    .iter()
                    .all(|s| matches!(s.descriptor, StreamDescriptor::Local(_))),
                "expected Local descriptor for track {:?}",
                item.title
            );
        }
    }

    // -----------------------------------------------------------------------
    // E2E: .strm files — the URL inside the file is stored as path, not the
    // filesystem path of the .strm file itself.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_strm_stores_url_as_path() {
        let url = "https://example.com/videos/matrix.mkv";
        let dir = tempfile::tempdir().unwrap();
        write_files(
            dir.path(),
            &[("[imdbid-tt0133093] The Matrix (1999).strm", url.as_bytes())],
        );

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let stored_path: String = sqlx::query_scalar(
            "SELECT path FROM opendal_files WHERE addon_id = ? AND imdb_id = 'tt0133093'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();

        assert_eq!(
            stored_path, url,
            ".strm path must be the URL from file contents"
        );

        // get_streams must also return the URL as the stream path.
        let stub = db::Media {
            id: common::get_stable_uuid("movie:tt0133093".to_string()),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0133093".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let streams = addon
            .get_streams(&stub, ctx, None)
            .await
            .unwrap();
        assert_eq!(streams.len(), 1);
        let path = match &streams[0].descriptor {
            StreamDescriptor::Local(p) => p
                .to_string_lossy()
                .to_string(),
            other => panic!("expected Local descriptor, got {other:?}"),
        };
        assert_eq!(path, url, "stream path must be the URL from the .strm file");
    }

    // -----------------------------------------------------------------------
    // Regression: two `.strm` files that resolve to the same URL must not
    // abort the whole scan with a UNIQUE(addon_id, path) error.
    //
    // `id` is derived from the .strm file's own filesystem path, while the
    // stored `path` is the URL read from inside the file — two different
    // files can therefore produce different ids but an identical path. That
    // used to hit a plain INSERT (ON CONFLICT(id) doesn't fire, since the
    // ids differ) which violated the UNIQUE(addon_id, path) index and
    // propagated an error out of the whole scan via `?`, which in turn meant
    // `prune_stale_paths` never ran — a single collision permanently wedged
    // every future scan of the addon on the same row.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_strm_url_collision_does_not_abort_scan() {
        let url = "https://example.com/videos/shared.mkv";
        let dir = tempfile::tempdir().unwrap();
        write_files(
            dir.path(),
            &[
                ("[imdbid-tt0133093] The Matrix (1999).strm", url.as_bytes()),
                ("[imdbid-tt0106977] Heat (1995).strm", url.as_bytes()),
            ],
        );

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;

        // Must complete without error even though the two files collide on path.
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        // The UNIQUE(addon_id, path) index means only one of the two rows can
        // hold that path — the other is skipped, not silently duplicated.
        let count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM opendal_files WHERE addon_id = ?")
                .bind(db_addon.id)
                .fetch_one(&ctx.db)
                .await
                .unwrap();
        assert_eq!(
            count, 1,
            "exactly one of the two colliding .strm files should be indexed"
        );

        // Running it again must still succeed (no permanent deadlock).
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
    }

    // -----------------------------------------------------------------------
    // Regression: a stale row from a file that has since been renamed (e.g.
    // VOD2MLIB regenerating a `.strm` under a new filename for the same
    // proxy URL) must eventually be cleared and the current file indexed —
    // not just "scan doesn't crash", but genuine recovery.
    //
    // A single rescan right after the rename still collides (the stale row
    // hasn't been pruned yet), so the new file's insert is skipped that
    // pass; the collision-skip fix lets the scan reach `prune_stale_paths`
    // regardless, which removes the stale row since its id is no longer
    // seen. The following rescan then has a clear path and indexes the
    // current file, preserving its IMDb id throughout.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_strm_rename_recovers_from_stale_row_collision() {
        let url = "https://example.com/videos/shared.mkv";
        let dir = tempfile::tempdir().unwrap();
        let old_name = "[imdbid-tt0133093] The Matrix (1999).strm";
        write_files(dir.path(), &[(old_name, url.as_bytes())]);

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;

        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let stale_id: Uuid = sqlx::query_scalar(
            "SELECT id FROM opendal_files WHERE addon_id = ? AND path = ?",
        )
        .bind(db_addon.id)
        .bind(url)
        .fetch_one(&ctx.db)
        .await
        .unwrap();

        // Simulate VOD2MLIB regenerating the `.strm` under a new filename
        // that still resolves to the same proxy URL — a different fs path
        // (and therefore a different derived id) colliding on `path`.
        std::fs::remove_file(
            dir.path()
                .join(old_name),
        )
        .unwrap();
        let new_name = "[imdbid-tt0133093] The Matrix (1999) [2160p].strm";
        write_files(dir.path(), &[(new_name, url.as_bytes())]);

        // First rescan: the stale row still occupies `path`, so the new
        // file's insert collides and is skipped — but must not abort, and
        // must still reach prune_stale_paths to clear the stale row out.
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let after_first_rescan: Vec<Uuid> =
            sqlx::query_scalar("SELECT id FROM opendal_files WHERE addon_id = ?")
                .bind(db_addon.id)
                .fetch_all(&ctx.db)
                .await
                .unwrap();
        assert!(
            !after_first_rescan.contains(&stale_id),
            "stale row must be pruned even though this pass's insert collided"
        );

        // Second rescan: no more collision, the current file is indexed.
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let (current_id, imdb_id, name): (Uuid, Option<String>, String) = sqlx::query_as(
            "SELECT id, imdb_id, name FROM opendal_files WHERE addon_id = ? AND path = ?",
        )
        .bind(db_addon.id)
        .bind(url)
        .fetch_one(&ctx.db)
        .await
        .unwrap();

        assert_ne!(
            current_id, stale_id,
            "surviving row must be the current file, not the stale one"
        );
        assert_eq!(name, new_name);
        assert_eq!(imdb_id.as_deref(), Some("tt0133093"));
    }

    // -----------------------------------------------------------------------
    // E2E: stale video rows are pruned when files are deleted and re-indexed.
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_stale_video_prune() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join("[imdbid-tt0133093] The Matrix (1999).mkv");
        std::fs::write(&file, b"fake").unwrap();

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files WHERE addon_id = ? AND media_kind = 'movie'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(count, 1, "file should be indexed on first scan");

        std::fs::remove_file(&file).unwrap();
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM opendal_files WHERE addon_id = ? AND media_kind = 'movie'",
        )
        .bind(db_addon.id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(
            count, 0,
            "stale video row must be pruned after file is deleted"
        );
    }

    // ---------------------------------------------------------------------------
    // Regression: episode files whose filename starts with "S01E07 - Episode Title"
    // must store the *series* name (from the parent directory) as their title,
    // not the episode title extracted from the filename.
    //
    // Reproduces: user report where "Wallace & Gromit's Cracking Contraptions"
    // appeared as "S01E07 - The 525 CrackerVac" in the catalog.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_episode_title_comes_from_series_dir_not_filename() {
        // File that starts with SxxExx — hunch parses nothing useful as a series
        // title from the filename alone; the series name lives in the parent dir.
        let rel_path = "Wallace & Gromit's Cracking Contraptions (2002) [imdbid-tt0103584]/\
                        Season 01/\
                        S01E07 - The 525 CrackerVac [DVD][AC3 2.0][h265].mkv";

        let dir = tempfile::tempdir().unwrap();
        write_files(&dir.path(), &[(rel_path, b"fake")]);

        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let (addon, db_addon) = make_local_addon(ctx, dir.path(), "episode").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();

        let row: Option<(Option<String>, Option<i64>, Option<i64>)> = sqlx::query_as(
            "SELECT title, season, episode FROM opendal_files \
             WHERE addon_id = ? AND media_kind = 'episode' AND imdb_id = 'tt0103584'",
        )
        .bind(db_addon.id)
        .fetch_optional(&ctx.db)
        .await
        .unwrap();

        let (title, season, episode) = row.expect(
            "expected one opendal_files row for imdbid-tt0103584 — file was not indexed",
        );

        assert_eq!(season, Some(1), "season must be 1");
        assert_eq!(episode, Some(7), "episode must be 7");

        let title = title.unwrap_or_default();
        // The series directory is the source of truth for the title.
        // It must NOT contain the episode-filename portion ("The 525 CrackerVac")
        // and must contain the actual series name.
        assert!(
            !title
                .to_lowercase()
                .contains("crackvac")
                && !title
                    .to_lowercase()
                    .contains("crackervac")
                && !title
                    .to_lowercase()
                    .contains("s01e07"),
            "title must not be the episode filename; got: {title:?}"
        );
        assert!(
            title
                .to_lowercase()
                .contains("wallace"),
            "title must contain the series name from the directory; got: {title:?}"
        );
    }

    // --- probe on scan -------------------------------------------------

    const PROBE_MOVIE: &str = "[imdbid-tt0133093] The Matrix (1999).mp4";

    fn make_probeable_mp4(path: &std::path::Path) -> bool {
        std::process::Command::new(
            std::env::var("FFMPEG_PATH").unwrap_or_else(|_| "ffmpeg".into()),
        )
        .args([
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc=duration=1:size=64x64:rate=5",
            "-pix_fmt",
            "yuv420p",
            "-y",
        ])
        .arg(path)
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    }

    async fn stored_probe(ctx: &AppContext, addon_id: Uuid) -> bool {
        sqlx::query_scalar::<_, bool>(
            "SELECT probe_data IS NOT NULL FROM opendal_files WHERE addon_id = ?",
        )
        .bind(addon_id)
        .fetch_one(&ctx.db)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn probe_on_scan_stores_probe_data_and_serves_it() {
        let dir = tempfile::tempdir().unwrap();
        if !make_probeable_mp4(
            &dir.path()
                .join(PROBE_MOVIE),
        ) {
            eprintln!("ffmpeg unavailable, skipping");
            return;
        }
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, mut db_addon) = make_local_addon(ctx, dir.path(), "movie").await;

        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(!stored_probe(ctx, db_addon.id).await, "off by default");

        db_addon.probe_on_scan = true;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(stored_probe(ctx, db_addon.id).await);

        let media = db::Media {
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0133093".to_string()).ok(),
                ..Default::default()
            },
            ..Default::default()
        };
        let streams = addon
            .get_streams(&media, ctx, None)
            .await
            .unwrap();
        assert_eq!(streams.len(), 1);
        assert!(
            streams[0]
                .probe_data
                .as_ref()
                .and_then(|p| p.video_stream())
                .is_some()
        );
    }

    #[tokio::test]
    async fn probe_on_scan_drops_stale_probe_when_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join(PROBE_MOVIE);
        if !make_probeable_mp4(&file) {
            eprintln!("ffmpeg unavailable, skipping");
            return;
        }
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, mut db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        db_addon.probe_on_scan = true;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(stored_probe(ctx, db_addon.id).await);

        std::fs::write(&file, b"no longer a video").unwrap();
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(!stored_probe(ctx, db_addon.id).await);
    }

    #[tokio::test]
    async fn probing_disabled_keeps_probe_until_file_changes() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join(PROBE_MOVIE);
        if !make_probeable_mp4(&file) {
            eprintln!("ffmpeg unavailable, skipping");
            return;
        }
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, mut db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        db_addon.probe_on_scan = true;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(stored_probe(ctx, db_addon.id).await);

        db_addon.probe_on_scan = false;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(stored_probe(ctx, db_addon.id).await);

        std::fs::write(&file, b"no longer a video").unwrap();
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(!stored_probe(ctx, db_addon.id).await);
    }

    #[tokio::test]
    async fn probe_on_scan_failure_does_not_abort_the_scan() {
        let dir = tempfile::tempdir().unwrap();
        write_files(dir.path(), &[(PROBE_MOVIE, b"fake")]);
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, mut db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        db_addon.probe_on_scan = true;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(!stored_probe(ctx, db_addon.id).await);
    }

    #[test]
    fn webdav_probe_job_encodes_path_and_adds_basic_auth() {
        let base = WebdavProbeBase::from_cfg(&serde_json::json!({
            "endpoint": "https://dav.example/remote.php/",
            "username": "u",
            "password": "p",
        }))
        .unwrap();
        let job = base.job(Uuid::nil(), "/Movies/The Matrix #1.mkv", String::new());
        assert_eq!(
            job.input
                .expose(),
            "https://dav.example/remote.php/Movies/The%20Matrix%20%231.mkv"
        );
        assert_eq!(
            job.headers
                .as_deref(),
            Some("Authorization: Basic dTpw\r\n")
        );
    }

    #[test]
    fn probe_version_changes_with_strm_target_even_at_same_size() {
        let meta = opendal::Metadata::new(EntryMode::FILE).with_content_length(24);
        let a = probe_version_of("http://host/aaaaaaaaa.mkv", &meta);
        let b = probe_version_of("http://host/bbbbbbbbb.mkv", &meta);
        assert_ne!(a, b);
        assert_eq!(a, probe_version_of("http://host/aaaaaaaaa.mkv", &meta));
        let tagged = meta
            .clone()
            .with_etag("\"v2\"".to_string());
        assert_ne!(a, probe_version_of("http://host/aaaaaaaaa.mkv", &tagged));
    }

    #[tokio::test]
    async fn probe_on_scan_drops_stale_probe_when_same_size_file_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir
            .path()
            .join(PROBE_MOVIE);
        if !make_probeable_mp4(&file) {
            eprintln!("ffmpeg unavailable, skipping");
            return;
        }
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, mut db_addon) = make_local_addon(ctx, dir.path(), "movie").await;
        db_addon.probe_on_scan = true;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(stored_probe(ctx, db_addon.id).await);

        let len = std::fs::metadata(&file)
            .unwrap()
            .len() as usize;
        tokio::time::sleep(Duration::from_millis(50)).await;
        std::fs::write(&file, vec![b'x'; len]).unwrap();
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        assert!(!stored_probe(ctx, db_addon.id).await);
    }

    // -----------------------------------------------------------------------
    // Local music (Jellyfin layout): Artist/Album/Track folders, album-only
    // folders and loose tracks. See
    // https://jellyfin.org/docs/general/server/media/music/
    // -----------------------------------------------------------------------

    async fn index_track_fixtures(
        ctx: &crate::AppContext,
        dir: &std::path::Path,
        fixtures: &[&str],
    ) -> (OpendalAddon, Addon) {
        write_files(
            dir,
            &fixtures
                .iter()
                .map(|p| (*p, b"fake" as &[u8]))
                .collect::<Vec<_>>(),
        );
        let (addon, db_addon) = make_local_addon(ctx, dir, "track").await;
        addon
            .refresh_index(ctx, &db_addon, noop_progress())
            .await
            .unwrap();
        (addon, db_addon)
    }

    /// Scan parses artist/album from the Jellyfin folder layout and track
    /// number + title from the filename.
    #[tokio::test]
    async fn opendal_track_scan_parses_jellyfin_music_layout() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            // {Artist}/{Album}/{NN - Title}
            "Steely Dan/Two Against Nature/03 - Two Against Nature.flac",
            // album-only folder; without embedded tags the folder name is the album
            "Incubus - Morning View XXIII (2024) [FLAC]/03. Wish You Were Here.flac",
            // {Artist}/{Album}/{NN Title}
            "Various Artists/Now That's What I Call Music/01 Uptown Funk.mp3",
            // loose track at the root
            "Loose Track.mp3",
        ];
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (_, db_addon) = index_track_fixtures(ctx, dir.path(), &fixtures).await;

        #[derive(Debug, sqlx::FromRow)]
        struct Row {
            path: String,
            title: Option<String>,
            track_number: Option<i64>,
            artist: Option<String>,
            album: Option<String>,
        }
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT path, title, track_number, artist, album FROM opendal_files              WHERE addon_id = ? AND media_kind = 'track' ORDER BY path",
        )
        .bind(db_addon.id)
        .fetch_all(&ctx.db)
        .await
        .unwrap();

        assert_eq!(
            rows.len(),
            4,
            "every fixture file must be indexed; got: {rows:?}"
        );
        for r in &rows {
            let rel = r
                .path
                .rsplit('/')
                .next()
                .unwrap();
            match rel {
                "03 - Two Against Nature.flac" => {
                    assert_eq!(
                        r.artist
                            .as_deref(),
                        Some("Steely Dan")
                    );
                    assert_eq!(
                        r.album
                            .as_deref(),
                        Some("Two Against Nature")
                    );
                    assert_eq!(
                        r.title
                            .as_deref(),
                        Some("Two Against Nature")
                    );
                    assert_eq!(r.track_number, Some(3));
                }
                "03. Wish You Were Here.flac" => {
                    // album-only folder: the folder name is the album; artist is
                    // unknown without embedded tags
                    assert_eq!(
                        r.album
                            .as_deref(),
                        Some("Incubus - Morning View XXIII (2024) [FLAC]")
                    );
                    assert_eq!(r.artist, None);
                    assert_eq!(
                        r.title
                            .as_deref(),
                        Some("Wish You Were Here")
                    );
                    assert_eq!(r.track_number, Some(3));
                }
                "01 Uptown Funk.mp3" => {
                    assert_eq!(
                        r.artist
                            .as_deref(),
                        Some("Various Artists")
                    );
                    assert_eq!(
                        r.album
                            .as_deref(),
                        Some("Now That's What I Call Music")
                    );
                    assert_eq!(
                        r.title
                            .as_deref(),
                        Some("Uptown Funk")
                    );
                    assert_eq!(r.track_number, Some(1));
                }
                "Loose Track.mp3" => {
                    assert_eq!(r.artist, None);
                    assert_eq!(r.album, None);
                    assert_eq!(
                        r.title
                            .as_deref(),
                        Some("Loose Track")
                    );
                    assert_eq!(r.track_number, None);
                }
                other => panic!("unexpected fixture path: {other}"),
            }
        }
    }

    /// Catalog builds the Artist -> Album -> Track hierarchy with per-file ids:
    /// tracks with the same title in different albums/artists must not collide.
    #[tokio::test]
    async fn opendal_track_catalog_builds_music_hierarchy() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            "Artist A/Album One/Intro.flac",
            "Artist A/Album Two/Intro.flac",
            "Artist B/Album One/Intro.flac",
        ];
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, _) = index_track_fixtures(ctx, dir.path(), &fixtures).await;

        let catalog: Vec<db::Media> = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap()
            .collect()
            .await;

        let tracks: Vec<&db::Media> = catalog
            .iter()
            .filter(|m| m.kind == db::MediaKind::Track)
            .collect();
        let albums: Vec<&db::Media> = catalog
            .iter()
            .filter(|m| m.kind == db::MediaKind::Album)
            .collect();
        let artists: Vec<&db::Media> = catalog
            .iter()
            .filter(|m| m.kind == db::MediaKind::Artist)
            .collect();

        assert_eq!(
            tracks.len(),
            3,
            "each file must produce its own track; got: {:?}",
            tracks
                .iter()
                .map(|m| &m.title)
                .collect::<Vec<_>>()
        );
        let track_ids: std::collections::HashSet<_> = tracks
            .iter()
            .map(|m| m.id)
            .collect();
        assert_eq!(
            track_ids.len(),
            3,
            "same-titled tracks in different albums/artists must have distinct ids"
        );

        assert_eq!(albums.len(), 3, "one album row per distinct artist+album");
        assert_eq!(artists.len(), 2, "one artist row per distinct artist");

        for t in &tracks {
            assert!(
                t.parent_id
                    .is_some(),
                "track must link to its album"
            );
            assert!(
                t.grandparent_id
                    .is_some(),
                "track must link to its artist"
            );
        }
    }

    /// Streams resolve to the exact file for the track, not a same-titled file
    /// from another album.
    #[tokio::test]
    async fn opendal_track_streams_resolve_exact_file() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            "Artist A/Album One/Intro.flac",
            "Artist A/Album Two/Intro.flac",
            "Artist B/Album One/Intro.flac",
        ];
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, _) = index_track_fixtures(ctx, dir.path(), &fixtures).await;

        let catalog: Vec<db::Media> = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap()
            .collect()
            .await;

        let artist_a_id: uuid::Uuid = catalog
            .iter()
            .find(|m| m.kind == db::MediaKind::Artist && m.title == "Artist A")
            .expect("Artist A row in catalog")
            .id;
        let target = catalog
            .iter()
            .find(|m| {
                m.kind == db::MediaKind::Track
                    && m.title == "Intro"
                    && m.grandparent_id == Some(artist_a_id)
            })
            .expect("track under Artist A");

        let streams = addon
            .get_streams(target, ctx, None)
            .await
            .unwrap();
        assert_eq!(streams.len(), 1, "exactly one stream for the track");
        match &streams[0].descriptor {
            crate::stream::StreamDescriptor::Local(p) => {
                assert!(
                    p.to_string_lossy()
                        .ends_with("Artist A/Album One/Intro.flac"),
                    "must resolve the exact file; got: {p:?}"
                );
            }
            other => panic!("expected Local descriptor, got {other:?}"),
        }
    }

    #[test]
    fn music_layout_follows_jellyfin_folders() {
        let l = |p: &str, d: &str| music_layout(p, d);
        let some = |s: &str| Some(s.to_string());
        assert_eq!(
            l("Steely Dan/Two Against Nature/03 - Two.flac", "/"),
            (some("Steely Dan"), some("Two Against Nature"), None)
        );
        // album-only folder and a loose track
        assert_eq!(l("Album/01 Song.flac", "/"), (None, some("Album"), None));
        assert_eq!(l("Song.flac", "/"), (None, None, None));
        // WebDAV paths are relative to the operator root, not the scan directory
        assert_eq!(
            l("Music/Steely Dan/Two Against Nature/03.flac", "/Music/"),
            (some("Steely Dan"), some("Two Against Nature"), None)
        );
        // `Disc N` / `CD N` folders carry the disc number; look-alike album names do not
        assert_eq!(
            l("Artist/Album/Disc 2/01.flac", "/"),
            (some("Artist"), some("Album"), Some(2))
        );
        assert_eq!(
            l("Artist/Album/CD1/01.flac", "/"),
            (some("Artist"), some("Album"), Some(1))
        );
        assert_eq!(
            l("Artist/CD Singles/01 Song.flac", "/"),
            (some("Artist"), some("CD Singles"), None)
        );
        assert_eq!(
            l("Disc World/01 Song.flac", "/"),
            (None, some("Disc World"), None)
        );
    }

    /// A `Disc N` folder is not an album: both discs share the album, and each
    /// track carries its disc number so clients can tell `01` on disc 1 from
    /// `01` on disc 2.
    #[tokio::test]
    async fn opendal_track_catalog_keeps_disc_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let fixtures = [
            "Artist/Album/Disc 1/01 Song.flac",
            "Artist/Album/Disc 2/01 Song.flac",
        ];
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, _) = index_track_fixtures(ctx, dir.path(), &fixtures).await;

        let catalog: Vec<db::Media> = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap()
            .collect()
            .await;
        let albums = catalog
            .iter()
            .filter(|m| m.kind == db::MediaKind::Album)
            .count();
        assert_eq!(albums, 1, "both discs belong to one album");
        let mut discs: Vec<(Option<i64>, Option<i64>)> = catalog
            .iter()
            .filter(|m| m.kind == db::MediaKind::Track)
            .map(|m| (m.parent_idx, m.idx))
            .collect();
        discs.sort();
        assert_eq!(discs, vec![(Some(1), Some(1)), (Some(2), Some(1))]);
    }

    /// Every music addon supports every track, so a track's stored stream must
    /// only be served by the addon that indexed that file.
    #[tokio::test]
    async fn opendal_track_stream_belongs_to_the_indexing_addon() {
        let dir_a = tempfile::tempdir().unwrap();
        let dir_b = tempfile::tempdir().unwrap();
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon_a, _) =
            index_track_fixtures(ctx, dir_a.path(), &["Artist/Album/Song.flac"]).await;
        let (addon_b, _) = index_track_fixtures(
            ctx,
            dir_b.path(),
            &["Other/Elsewhere/Different.flac"],
        )
        .await;

        let catalog: Vec<db::Media> = addon_a
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap()
            .collect()
            .await;
        let track = catalog
            .iter()
            .find(|m| m.kind == db::MediaKind::Track)
            .unwrap();

        assert_eq!(
            addon_a
                .get_streams(track, ctx, None)
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(
            addon_b
                .get_streams(track, ctx, None)
                .await
                .unwrap()
                .is_empty(),
            "another addon must not serve this addon's file"
        );
    }

    #[test]
    fn music_album_keys_are_unambiguous() {
        let addon = Uuid::nil();
        assert_ne!(
            music_album_key(addon, Some("A:B"), "C"),
            music_album_key(addon, Some("A"), "B:C")
        );
        assert_ne!(
            music_album_key(addon, None, "A:B"),
            music_album_key(addon, Some("A"), "B")
        );
        assert_eq!(
            music_album_key(addon, Some("A"), "B"),
            music_album_key(addon, Some("A"), "B")
        );
    }

    /// Playback resolves from the row the import persisted, not from the
    /// in-memory catalog item: the stored track keeps its stream, so two
    /// identically titled tracks in different albums play their own files.
    #[tokio::test]
    async fn opendal_persisted_tracks_resolve_their_own_files() {
        let dir = tempfile::tempdir().unwrap();
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let (addon, db_addon) = index_track_fixtures(
            ctx,
            dir.path(),
            &["Artist/Album One/Intro.flac", "Artist/Album Two/Intro.flac"],
        )
        .await;

        let collection_id = Uuid::new_v4();
        let catalog = crate::addons::ResolvedCatalog {
            provider_catalog_id: "files".into(),
            catalog_id: format!("addon:{}:files", db_addon.id),
            collection_id,
            name: "files".into(),
            media_kind: Some(db::MediaKind::Track),
            collection_media_kind: None,
            enabled: true,
            max_items: None,
            tags: vec![],
        };
        db::Media {
            id: collection_id,
            kind: db::MediaKind::Collection,
            title: "files".into(),
            ..Default::default()
        }
        .save(&ctx.db)
        .await
        .unwrap();
        let stream = addon
            .catalog_stream(ctx, "files")
            .await
            .unwrap()
            .unwrap();
        crate::tasks::catalog_import_shared::import_catalog_items(
            ctx,
            &catalog,
            &catalog.catalog_id,
            100,
            stream,
            &noop_progress(),
        )
        .await
        .unwrap();

        let track_ids: Vec<Uuid> = sqlx::query_scalar(
            "SELECT id FROM media WHERE CAST(kind AS TEXT) = 'track'",
        )
        .fetch_all(&ctx.db)
        .await
        .unwrap();
        assert_eq!(track_ids.len(), 2, "both same-titled tracks are persisted");

        let mut resolved = Vec::new();
        for id in track_ids {
            let track = db::Media::get_by_id(&ctx.db, &id)
                .await
                .unwrap()
                .unwrap();
            let streams = addon
                .get_streams(&track, ctx, None)
                .await
                .unwrap();
            assert_eq!(streams.len(), 1, "one stream per persisted track");
            match &streams[0].descriptor {
                crate::stream::StreamDescriptor::Local(p) => {
                    resolved.push(
                        p.to_string_lossy()
                            .into_owned(),
                    );
                }
                other => panic!("expected Local descriptor, got {other:?}"),
            }
        }
        resolved.sort();
        assert!(
            resolved[0].ends_with("Artist/Album One/Intro.flac"),
            "{resolved:?}"
        );
        assert!(
            resolved[1].ends_with("Artist/Album Two/Intro.flac"),
            "{resolved:?}"
        );
    }

    // -----------------------------------------------------------------------
    // Incremental WebDAV scans: a folder that has not changed since it was
    // indexed is not listed again.
    // -----------------------------------------------------------------------

    const OLD: &str = "Mon, 01 Jan 2024 00:00:00 GMT";
    const FUTURE: &str = "Fri, 01 Jan 2100 00:00:00 GMT";
    const MOVIE: &str = "%5Bimdbid-tt0133093%5D%20The%20Matrix%20(1999).mkv";
    const MOVIE2: &str = "%5Bimdbid-tt0106977%5D%20Heat%20(1995).mkv";

    type DavRoutes = Arc<std::sync::Mutex<std::collections::HashMap<String, String>>>;
    type DavHits = Arc<std::sync::Mutex<Vec<String>>>;

    fn dav_entry(href: &str, dir: bool, modified: &str) -> String {
        let rt = if dir {
            "<D:resourcetype><D:collection/></D:resourcetype>"
        } else {
            "<D:resourcetype/><D:getcontentlength>5</D:getcontentlength>"
        };
        format!(
            "<D:response><D:href>{href}</D:href><D:propstat><D:prop>{rt}<D:getlastmodified>{modified}</D:getlastmodified></D:prop><D:status>HTTP/1.1 200 OK</D:status></D:propstat></D:response>"
        )
    }

    fn dav_body(entries: &[String]) -> String {
        format!(
            r#"<?xml version="1.0"?><D:multistatus xmlns:D="DAV:">{}</D:multistatus>"#,
            entries.concat()
        )
    }

    /// PROPFIND mock whose routes can be edited between scans. Records every
    /// requested path.
    async fn mutable_webdav() -> (String, DavRoutes, DavHits) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap();
        let base = format!(
            "http://{}",
            listener
                .local_addr()
                .unwrap()
        );
        let routes: DavRoutes = Default::default();
        let hits: DavHits = Default::default();
        let (r, h) = (routes.clone(), hits.clone());
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = listener
                    .accept()
                    .await
                    .unwrap();
                let (routes, hits) = (r.clone(), h.clone());
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let n = sock
                        .read(&mut buf)
                        .await
                        .unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]).to_string();
                    let path = req
                        .split_whitespace()
                        .nth(1)
                        .unwrap_or("")
                        .to_string();
                    hits.lock()
                        .unwrap()
                        .push(path.clone());
                    let body = routes
                        .lock()
                        .unwrap()
                        .get(&path)
                        .cloned();
                    let (status, body) = match body {
                        Some(b) => ("207 Multi-Status", b),
                        None => ("404 Not Found", String::new()),
                    };
                    let resp = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock
                        .write_all(resp.as_bytes())
                        .await;
                });
            }
        });
        (base, routes, hits)
    }

    /// Registers a folder's own listing and returns its entry for the root listing.
    fn set_dav_dir(
        routes: &DavRoutes,
        name: &str,
        modified: &str,
        files: &[&str],
    ) -> String {
        let mut entries = vec![dav_entry(&format!("/{name}/"), true, modified)];
        entries.extend(
            files
                .iter()
                .map(|f| dav_entry(&format!("/{name}/{f}"), false, modified)),
        );
        routes
            .lock()
            .unwrap()
            .insert(format!("/{name}/"), dav_body(&entries));
        dav_entry(&format!("/{name}/"), true, modified)
    }

    fn set_dav_root(routes: &DavRoutes, dirs: &[String]) {
        let mut entries = vec![dav_entry("/", true, OLD)];
        entries.extend_from_slice(dirs);
        routes
            .lock()
            .unwrap()
            .insert("/".to_string(), dav_body(&entries));
    }

    async fn make_webdav_addon(ctx: &AppContext, endpoint: &str) -> Addon {
        let now = Utc::now().naive_utc();
        let addon = Addon {
            id: Uuid::new_v4(),
            name: "test-webdav".to_string(),
            preset: AddonPresetRef {
                kind: "opendal-webdav".to_string(),
                config: serde_json::json!({
                    "media_kind": "movie",
                    "endpoint": endpoint,
                    "paths": ["/"],
                    "incremental_scan": true,
                })
                .into(),
            },
            resources: vec![ResourceType::Stream, ResourceType::Catalog],
            types: vec![],
            enabled: true,
            priority: 0,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            subtitle_extraction: false,
            probe_on_scan: false,
            service_filter: vec![],
            created_at: now,
            updated_at: now,
        };
        addon
            .insert(&ctx.db)
            .await
            .unwrap();
        addon
    }

    async fn indexed_paths(ctx: &AppContext, addon: &Addon) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT path FROM opendal_files WHERE addon_id = ? ORDER BY path",
        )
        .bind(addon.id)
        .fetch_all(&ctx.db)
        .await
        .unwrap()
    }

    /// Two folders indexed by a first scan; the request log is cleared after it.
    async fn scanned_share() -> (
        crate::integration_test::TestGuard,
        Addon,
        DavRoutes,
        DavHits,
    ) {
        let (base, routes, hits) = mutable_webdav().await;
        let a = set_dav_dir(&routes, "A", OLD, &[MOVIE]);
        let b = set_dav_dir(&routes, "B", OLD, &[MOVIE2]);
        set_dav_root(&routes, &[a, b]);
        let (_, guard) = new_test_server()
            .await
            .unwrap();
        let addon = make_webdav_addon(&guard.0, &base).await;
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        assert_eq!(
            indexed_paths(&guard.0, &addon)
                .await
                .len(),
            2
        );
        hits.lock()
            .unwrap()
            .clear();
        (guard, addon, routes, hits)
    }

    #[tokio::test]
    async fn webdav_rescan_skips_unchanged_folders() {
        let (guard, addon, _routes, hits) = scanned_share().await;
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        assert_eq!(
            *hits
                .lock()
                .unwrap(),
            vec!["/".to_string()],
            "unchanged folders must not be listed"
        );
        assert_eq!(
            indexed_paths(&guard.0, &addon)
                .await
                .len(),
            2,
            "rows of skipped folders must survive the prune"
        );
    }

    #[tokio::test]
    async fn webdav_rescan_indexes_new_folder() {
        let (guard, addon, routes, hits) = scanned_share().await;
        let a = set_dav_dir(&routes, "A", OLD, &[MOVIE]);
        let b = set_dav_dir(&routes, "B", OLD, &[MOVIE2]);
        let c = set_dav_dir(&routes, "C", OLD, &[MOVIE]);
        set_dav_root(&routes, &[a, b, c]);
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        let hits = hits
            .lock()
            .unwrap()
            .clone();
        assert!(hits.contains(&"/C/".to_string()), "{hits:?}");
        assert!(!hits.contains(&"/A/".to_string()), "{hits:?}");
        assert!(!hits.contains(&"/B/".to_string()), "{hits:?}");
        assert_eq!(
            indexed_paths(&guard.0, &addon)
                .await
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn webdav_rescan_prunes_removed_folder() {
        let (guard, addon, routes, _hits) = scanned_share().await;
        let a = set_dav_dir(&routes, "A", OLD, &[MOVIE]);
        set_dav_root(&routes, &[a]);
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        assert_eq!(
            indexed_paths(&guard.0, &addon).await,
            vec!["A/[imdbid-tt0133093] The Matrix (1999).mkv".to_string()]
        );
    }

    #[tokio::test]
    async fn webdav_rescan_walks_folder_with_newer_mtime() {
        let (guard, addon, routes, hits) = scanned_share().await;
        let a = set_dav_dir(&routes, "A", FUTURE, &[MOVIE, MOVIE2]);
        let b = set_dav_dir(&routes, "B", OLD, &[MOVIE2]);
        set_dav_root(&routes, &[a, b]);
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        let hits = hits
            .lock()
            .unwrap()
            .clone();
        assert!(hits.contains(&"/A/".to_string()), "{hits:?}");
        assert!(!hits.contains(&"/B/".to_string()), "{hits:?}");
        assert_eq!(
            indexed_paths(&guard.0, &addon)
                .await
                .len(),
            3
        );
    }

    #[tokio::test]
    async fn webdav_rescan_walks_everything_by_default() {
        let (guard, mut addon, _routes, hits) = scanned_share().await;
        addon
            .preset
            .config = serde_json::json!({
            "media_kind": "movie",
            "endpoint": addon.preset.config.expose()["endpoint"],
            "paths": ["/"],
        })
        .into();
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        let hits = hits
            .lock()
            .unwrap()
            .clone();
        assert!(hits.contains(&"/A/".to_string()), "{hits:?}");
        assert!(hits.contains(&"/B/".to_string()), "{hits:?}");
    }

    #[tokio::test]
    async fn webdav_rescan_walks_everything_after_a_week() {
        let (guard, addon, _routes, hits) = scanned_share().await;
        sqlx::query("UPDATE opendal_files SET scanned_at = ? WHERE path LIKE 'A/%'")
            .bind((Utc::now().naive_utc() - chrono::Duration::days(8)).to_string())
            .execute(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        let hits = hits
            .lock()
            .unwrap()
            .clone();
        assert!(hits.contains(&"/A/".to_string()), "{hits:?}");
        assert!(hits.contains(&"/B/".to_string()), "{hits:?}");
    }

    #[tokio::test]
    async fn webdav_rescan_walks_folder_with_unresolved_row() {
        let (guard, addon, _routes, hits) = scanned_share().await;
        sqlx::query("UPDATE opendal_files SET imdb_id = NULL WHERE path LIKE 'A/%'")
            .execute(
                &guard
                    .0
                    .db,
            )
            .await
            .unwrap();
        scan_addon(&guard.0, &None, &addon)
            .await
            .unwrap();
        let hits = hits
            .lock()
            .unwrap()
            .clone();
        assert!(hits.contains(&"/A/".to_string()), "{hits:?}");
        assert!(!hits.contains(&"/B/".to_string()), "{hits:?}");
    }

    #[test]
    fn incremental_index_finds_only_rows_below_the_folder() {
        let at = Utc::now().naive_utc() - chrono::Duration::days(1);
        let row = |path: &str| IndexedRow {
            id: Uuid::new_v4(),
            path: path.to_string(),
            scanned_at: at,
            unresolved: false,
        };
        let (a, ab, b) = (
            row("Movies/A/x.mkv"),
            row("Movies/AB/x.mkv"),
            row("Movies/B/x.mkv"),
        );
        let a_id = a.id;
        let mut rows = vec![b, a, ab];
        rows.sort_unstable_by(|x, y| {
            x.path
                .cmp(&y.path)
        });
        let index = IncrementalIndex { rows };
        let old = Some(Utc::now() - chrono::Duration::days(30));
        assert_eq!(index.unchanged_rows("Movies/A/", old), Some(vec![a_id]));
        assert_eq!(index.unchanged_rows("Movies/C/", old), None);
        assert_eq!(index.unchanged_rows("Movies/A/", None), None);
    }
}
