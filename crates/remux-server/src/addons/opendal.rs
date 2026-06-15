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
    SubtitleInfo, TreeAddon,
};
use crate::{AppContext, addons::Addon, common, db, sdks, sdks::CachedEndpoint};

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
            supported_resources: vec![ResourceType::Stream, ResourceType::Catalog],
            supported_types: vec![
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
            supported_resources: vec![ResourceType::Stream, ResourceType::Catalog],
            supported_types: vec![
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
}

#[async_trait]
impl AddonKind for OpendalAddon {
    fn id(&self) -> &'static str {
        "opendal"
    }

    async fn available_info(
        &self,
    ) -> Result<Option<(Vec<ResourceType>, Vec<StremioMediaType>)>> {
        let media_type = match self
            .media_kind
            .as_str()
        {
            "episode" => StremioMediaType::Series,
            "track" => StremioMediaType::Track,
            _ => StremioMediaType::Movie,
        };
        Ok(Some((
            vec![ResourceType::Stream, ResourceType::Catalog],
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
                sqlx::query_as::<_, (Option<String>,)>(
                    "SELECT title FROM opendal_files \
                     WHERE addon_id = ? AND media_kind = 'track'",
                )
                .bind(self.addon_id)
                .fetch_all(&ctx.db)
                .await?
                .into_iter()
                .filter_map(|(title,)| title)
                .map(|title| db::Media {
                    id: common::get_stable_uuid(format!(
                        "{}:track:{}",
                        self.addon_id, title
                    )),
                    title: title.clone(),
                    kind: db::MediaKind::Track,
                    ..Default::default()
                })
                .collect()
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
        let tmdb = common::tmdb_client(&ctx.db).await;
        scan_addon(ctx, &tmdb, addon).await?;
        progress.set(100.0);
        Ok(())
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
                        .external_ids
                        .series_imdb
                        .is_some()
            }
            _ => false,
        }
    }

    async fn subtitle_fetch(
        &self,
        media: &db::Media,
        db: &sqlx::SqlitePool,
    ) -> Result<Vec<SubtitleInfo>> {
        #[derive(sqlx::FromRow)]
        struct SubRow {
            id: Uuid,
            path: String,
            name: String,
        }

        let rows: Vec<SubRow> = if self.media_kind == "episode" {
            let Some(imdb_id) = media
                .external_ids
                .series_imdb
                .as_deref()
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
                "SELECT id, path, name FROM opendal_files \
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
                "SELECT id, path, name FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'subtitle' AND imdb_id = ?",
            )
            .bind(self.addon_id)
            .bind(imdb_id)
            .fetch_all(db)
            .await?
        };

        Ok(rows
            .into_iter()
            .map(|r| {
                let stem = stem_without_ext(&r.name);
                let (_, lang, is_forced, is_hi) = split_subtitle_stem(&stem);
                SubtitleInfo {
                    id: r
                        .id
                        .to_string(),
                    url: Some(crate::stream::StreamDescriptor::Opendal {
                        addon_id: self.addon_id,
                        path: r.path,
                    }),
                    lang,
                    is_forced,
                    is_hi,
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
                        .external_ids
                        .series_imdb
                        .is_some()
            }
            "track" => media.kind == db::MediaKind::Track,
            _ => false,
        }
    }

    async fn get_streams(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<crate::stream::StreamInfo>> {
        let files: Vec<OpendalFile> = if self.media_kind == "track" {
            sqlx::query_as(
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size \
                 FROM opendal_files \
                 WHERE addon_id = ? AND media_kind = 'track' AND LOWER(title) = LOWER(?)",
            )
            .bind(self.addon_id)
            .bind(&media.title)
            .fetch_all(&ctx.db)
            .await?
        } else {
            // Episodes are identified by series_imdb (the show's IMDB ID scraped from the
            // filename tag); movies use their own imdb directly.
            let imdb_id = if self.media_kind == "episode" {
                media
                    .external_ids
                    .series_imdb
                    .as_deref()
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
                "SELECT path, name, title, imdb_id, season, episode, track_number, year, size \
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

                let seasons = season_nums
                    .into_iter()
                    .map(|s| db::Media {
                        id: common::get_stable_uuid(format!(
                            "season:{}:{}",
                            imdb_id, s
                        )),
                        title: format!("Season {}", s),
                        kind: db::MediaKind::Season,
                        parent_id: Some(root.id),
                        grandparent_id: Some(root.id),
                        idx: Some(s),
                        parent_idx: Some(s),
                        external_ids: db::ExternalIds {
                            series_imdb: db::NonEmptyString::try_new(
                                imdb_id.to_string(),
                            )
                            .ok(),
                            ..Default::default()
                        },
                        ..Default::default()
                    })
                    .collect();

                Ok(Some(seasons))
            }

            db::MediaKind::Season => {
                let Some(series_imdb) = root
                    .external_ids
                    .series_imdb
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
                            idx: Some(ep_num),
                            parent_idx: Some(season_num),
                            external_ids: db::ExternalIds {
                                series_imdb: db::NonEmptyString::try_new(
                                    series_imdb.to_string(),
                                )
                                .ok(),
                                ..Default::default()
                            },
                            stream_info: Some(crate::stream::StreamInfo {
                                descriptor,
                                name: Some(
                                    f.name
                                        .clone(),
                                ),
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

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "ts", "wmv", "webm", "strm",
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "ogg", "opus", "wav", "aac", "wv", "strm",
];

const SUBTITLE_EXTENSIONS: &[&str] = &["srt", "ass", "ssa", "vtt", "sub", "sup"];

/// Extract the file stem (filename without the last extension).
fn stem_without_ext(name: &str) -> String {
    match name.rsplit_once('.') {
        Some((stem, _)) => stem.to_string(),
        None => name.to_string(),
    }
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

async fn scan_addon(
    ctx: &AppContext,
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    addon: &Addon,
) -> Result<()> {
    let cfg = &addon
        .preset
        .config;
    let media_kind = cfg["media_kind"]
        .as_str()
        .unwrap_or("movie")
        .to_string();
    let is_local = addon
        .preset
        .kind
        == "opendal-local";

    info!(addon = %addon.name, kind = %addon.preset.kind, media_kind, "opendal: scanning");

    let extensions: &[&str] = if media_kind == "track" {
        AUDIO_EXTENSIONS
    } else {
        VIDEO_EXTENSIONS
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

    for (operator, list_from, path_prefix) in scan_roots {
        let mut lister = operator
            .lister_with(&list_from)
            .recursive(true)
            .await?;

        while let Some(entry) = lister
            .try_next()
            .await?
        {
            if entry
                .metadata()
                .mode()
                != EntryMode::FILE
            {
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
                                    crate::addons::tmdb::resolve_imdb_from_ids(
                                        &jellyfin_ids,
                                        true,
                                        client,
                                    )
                                    .await
                                    .map(Into::into)
                                } else {
                                    jellyfin_ids
                                        .imdb
                                        .clone()
                                        .map(Into::into)
                                }
                            } else {
                                resolve_imdb(tmdb, &clean_title, None, true).await
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
                                    crate::addons::tmdb::resolve_imdb_from_ids(
                                        &jellyfin_ids,
                                        false,
                                        client,
                                    )
                                    .await
                                    .map(Into::into)
                                } else {
                                    jellyfin_ids
                                        .imdb
                                        .clone()
                                        .map(Into::into)
                                }
                            } else {
                                resolve_imdb(tmdb, &clean_title, year, false).await
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

            if !extensions.contains(&ext.as_str()) {
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
                    SKIP_DIRS
                        .iter()
                        .any(|s| lower == *s)
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
                    let year = parsed
                        .year()
                        .map(|y| y as i64);
                    let clean_title = parsed
                        .title()
                        .unwrap_or(stem.as_str())
                        .to_string();

                    let existing_imdb =
                        fetch_existing_imdb(ctx, addon.id, &path).await?;
                    let imdb_id = if let Some(id) = existing_imdb {
                        Some(id)
                    } else if !jellyfin_ids.is_empty() {
                        if let Some(client) = tmdb {
                            crate::addons::tmdb::resolve_imdb_from_ids(
                                &jellyfin_ids,
                                true,
                                client,
                            )
                            .await
                            .map(Into::into)
                        } else {
                            jellyfin_ids
                                .imdb
                                .clone()
                                .map(Into::into)
                        }
                    } else {
                        resolve_imdb(tmdb, &clean_title, None, true).await
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
                    let clean_title = parsed
                        .title()
                        .unwrap_or(stem.as_str())
                        .to_string();

                    let existing_imdb =
                        fetch_existing_imdb(ctx, addon.id, &path).await?;
                    let imdb_id = if let Some(id) = existing_imdb {
                        Some(id)
                    } else if !jellyfin_ids.is_empty() {
                        if let Some(client) = tmdb {
                            crate::addons::tmdb::resolve_imdb_from_ids(
                                &jellyfin_ids,
                                false,
                                client,
                            )
                            .await
                            .map(Into::into)
                        } else {
                            jellyfin_ids
                                .imdb
                                .clone()
                                .map(Into::into)
                        }
                    } else {
                        resolve_imdb(tmdb, &clean_title, year, false).await
                    };

                    if imdb_id.is_none() {
                        debug!(path, title = %clean_title, "opendal: no IMDB id, skipping");
                        continue;
                    }

                    (Some(clean_title), None, None, None, year, imdb_id)
                }
            };

            let size = Some(
                entry
                    .metadata()
                    .content_length() as i64,
            );
            let now = Utc::now()
                .naive_utc()
                .to_string();

            sqlx::query(
                "INSERT INTO opendal_files \
                 (id, addon_id, media_kind, path, name, title, imdb_id, season, episode, track_number, year, size, scanned_at) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT(id) DO UPDATE SET \
                   path = excluded.path, \
                   name = excluded.name, media_kind = excluded.media_kind, \
                   title = excluded.title, \
                   imdb_id = COALESCE(opendal_files.imdb_id, excluded.imdb_id), \
                   season = excluded.season, episode = excluded.episode, \
                   track_number = excluded.track_number, \
                   year = excluded.year, size = excluded.size, scanned_at = excluded.scanned_at",
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
            .execute(&ctx.db)
            .await?;

            upserted += 1;
        }
    }

    let deleted = prune_stale_paths(ctx, addon.id, &seen_ids).await?;

    info!(
        addon = %addon.name,
        upserted,
        deleted,
        "opendal: scan complete"
    );

    Ok(())
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

async fn resolve_imdb(
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    title: &str,
    year: Option<i64>,
    is_tv: bool,
) -> Option<String> {
    let client = tmdb.as_ref()?;
    if title.is_empty() {
        return None;
    }

    if is_tv {
        let resp = client
            .execute(
                sdks::tmdb::SearchTvEndpoint {
                    query: title.to_string(),
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;
        let tmdb_id = resp
            .results
            .into_iter()
            .next()?
            .id;

        let series = client
            .execute(
                sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                    .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        series
            .external_ids
            .as_ref()
            .and_then(|e| {
                e.imdb_id
                    .clone()
            })
    } else {
        let resp = client
            .execute(
                sdks::tmdb::SearchMovieEndpoint {
                    query: title.to_string(),
                    year,
                }
                .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;
        let tmdb_id = resp
            .results
            .into_iter()
            .next()?
            .id;

        let movie = client
            .execute(
                sdks::tmdb::MovieEndpoint::new(tmdb_id)
                    .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        movie.imdb_id
    }
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
        addons::{Addon, AddonPresetRef},
        common::ProgressReporter,
        db,
        integration_test::new_test_server,
        stream::StreamDescriptor,
    };

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
                }),
            },
            resources: vec![ResourceType::Stream, ResourceType::Catalog],
            types: vec![],
            enabled: true,
            priority: 0,
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
                .get_streams(&stub, ctx)
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
        assert_eq!(series_items.len(), 3, "catalog should contain three Series");
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
                    .all(|s| s
                        .external_ids
                        .series_imdb
                        .as_deref()
                        == Some(imdb)),
                "{imdb}: Season items must carry series_imdb"
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
                        .all(|e| e
                            .external_ids
                            .series_imdb
                            .as_deref()
                            == Some(imdb)),
                    "{imdb} s{season_num}: Episode items must carry series_imdb"
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
    // Episode stream_supports + get_streams must use series_imdb, not imdb.
    // The existing tree-walk test only exercises get_children; this test covers
    // the separate path used when the server resolves streams for a known media row.
    // ---------------------------------------------------------------------------

    #[tokio::test]
    async fn opendal_episode_stream_supports_and_get_streams() {
        let fixtures: &[(&str, &str, i64, i64)] = &[
            // (rel_path, series_imdb, season, episode)
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

        // Walk Series → Season → Episodes to get real Episode rows with series_imdb set.
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
            // stream_supports must return true — this was the bug: it returned false when
            // it checked .imdb instead of .series_imdb.
            assert!(
                StreamAddon::supports(&addon, ep),
                "stream_supports must be true for episode with series_imdb set (e{:?})",
                ep.idx
            );

            let streams = addon
                .get_streams(ep, ctx)
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

        // Negative: an Episode row that only has `imdb` set (not `series_imdb`) must
        // return false — the opendal_files table stores series IMDB, not episode IMDB.
        let ep_with_imdb_only = db::Media {
            kind: db::MediaKind::Episode,
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0903747").ok(),
                series_imdb: None,
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(
            !StreamAddon::supports(&addon, &ep_with_imdb_only),
            "stream_supports must be false for episode with only imdb (not series_imdb)"
        );
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
            assert_eq!(
                parsed
                    .title()
                    .unwrap_or(""),
                c.title,
                "title mismatch for {:?}",
                c.stem
            );
            assert_eq!(parsed.year(), c.year, "year mismatch for {:?}", c.stem);
        }
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
            .subtitle_fetch(&movie_media, &ctx.db)
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
}
