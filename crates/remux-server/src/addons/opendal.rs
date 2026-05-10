use anyhow::{Result, bail};
use async_trait::async_trait;
use chrono::Utc;
use futures_util::TryStreamExt;
use opendal::EntryMode;
use regex::Regex;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use futures::Stream;
use remux_sdks::stremio::MediaType as StremioMediaType;
use tracing::{debug, info, warn};

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, AddonSelectOption, CatalogInfo, MediaKind,
    ProgressReporter, ResourceType,
};
use crate::addons::Addon;
use crate::sdks::CachedEndpoint;
use crate::{AppContext, common, db, sdks};

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
                    id: "path".to_string(),
                    name: "Path".to_string(),
                    description: Some(
                        "Absolute path to the root directory to scan.".to_string(),
                    ),
                    required: true,
                    default: None,
                    kind: AddonOptionType::String,
                },
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
        let media_kind = cfg["media_kind"].as_str().unwrap_or("movie").to_string();
        let path = cfg["path"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("opendal-local: path is required"))?;

        let operator =
            opendal::Operator::new(opendal::services::Fs::default().root(path))?
                .finish();

        Ok(Arc::new(OpendalAddon {
            addon_id,
            operator: Arc::new(operator),
            root: path.to_string(),
            backend: "local".to_string(),
            media_kind,
        }))
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
            ],
        }
    }

    fn from_cfg(
        &self,
        addon_id: Uuid,
        cfg: &serde_json::Value,
    ) -> Result<Arc<dyn AddonKind>> {
        let media_kind = cfg["media_kind"].as_str().unwrap_or("movie").to_string();
        let endpoint = cfg["endpoint"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("opendal-webdav: endpoint is required"))?;

        let mut builder = opendal::services::Webdav::default().endpoint(endpoint);
        if let Some(u) = cfg["username"].as_str().filter(|s| !s.is_empty()) {
            builder = builder.username(u);
        }
        if let Some(p) = cfg["password"].as_str().filter(|s| !s.is_empty()) {
            builder = builder.password(p);
        }
        let operator = opendal::Operator::new(builder)?.finish();

        Ok(Arc::new(OpendalAddon {
            addon_id,
            operator: Arc::new(operator),
            root: endpoint.to_string(),
            backend: "webdav".to_string(),
            media_kind,
        }))
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

    async fn available_info(&self) -> (Vec<ResourceType>, Vec<StremioMediaType>) {
        let media_type = match self.media_kind.as_str() {
            "episode" => StremioMediaType::Series,
            "track" => StremioMediaType::Track,
            _ => StremioMediaType::Movie,
        };
        (
            vec![ResourceType::Stream, ResourceType::Catalog],
            vec![media_type],
        )
    }

    async fn catalog_list(&self, _ctx: &AppContext) -> Result<Vec<CatalogInfo>> {
        Ok(vec![CatalogInfo {
            provider_catalog_id: "files".to_string(),
            name: "files".to_string(),
            default_enabled: true,
            default_max_items: Some(999999999),
        }])
    }

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
                        imdb: Some(imdb_id),
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
                        imdb: Some(imdb_id),
                        ..Default::default()
                    },
                    ..Default::default()
                })
                .collect()
            }
        };

        Ok(Some(Box::pin(futures::stream::iter(items))))
    }

    fn stream_supports(&self, media: &db::Media) -> bool {
        match self.media_kind.as_str() {
            "movie" => {
                media.kind == db::MediaKind::Movie && media.external_ids.imdb.is_some()
            }
            "episode" => {
                media.kind == db::MediaKind::Episode
                    && media.external_ids.imdb.is_some()
            }
            "track" => media.kind == db::MediaKind::Track,
            _ => false,
        }
    }

    async fn stream_resolve(
        &self,
        media: &db::Media,
        ctx: &AppContext,
    ) -> Result<Vec<db::Media>> {
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
            let imdb_id = match media.external_ids.imdb.as_deref() {
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
                    let ep_match =
                        media.idx.map(|e| f.episode == Some(e)).unwrap_or(true);
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
                let stable_id =
                    common::get_stable_uuid(format!("{}:{}", self.addon_id, f.path));
                let url = if self.backend == "local" {
                    let full = format!(
                        "{}/{}",
                        self.root.trim_end_matches('/'),
                        f.path.trim_start_matches('/')
                    );
                    Some(crate::stream::StreamDescriptor::Local(
                        std::path::PathBuf::from(full),
                    ))
                } else {
                    Some(crate::stream::StreamDescriptor::Opendal {
                        addon_id: self.addon_id,
                        path: f.path.clone(),
                    })
                };
                db::Media {
                    id: stable_id,
                    title: f.name.clone(),
                    kind: db::MediaKind::Stream,
                    url,
                    parent_id: Some(media.id),
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
        use axum::body::Body;
        use axum_anyhow::ResultExt;
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
            .context_not_found("stream", "file not found in opendal backend")?;
        let file_size = meta.content_length();
        let content_type = crate::stream::mime_from_path(std::path::Path::new(path));

        let range_str = headers
            .get(http::header::RANGE)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);

        if let Some(range) = range_str {
            let (start, end) = crate::stream::parse_range(&range, file_size)
                .context_bad_request("stream", "invalid Range header")?;
            let length = end - start + 1;

            let reader = self
                .operator
                .reader_with(path)
                .await
                .context_bad_request("stream", "failed to open opendal reader")?;
            let bytes_stream = reader
                .into_bytes_stream(start..start + length)
                .await
                .context_bad_request("stream", "failed to create opendal byte stream")?
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
                .context_bad_request("stream", "failed to open opendal reader")?;
            let bytes_stream = reader
                .into_bytes_stream(..)
                .await
                .context_bad_request("stream", "failed to create opendal byte stream")?
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

// ---------------------------------------------------------------------------
// Opendal file index scanning (backing refresh_index)
// ---------------------------------------------------------------------------

const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "avi", "mov", "m4v", "ts", "wmv", "webm", "strm",
];

const AUDIO_EXTENSIONS: &[&str] = &[
    "mp3", "flac", "m4a", "ogg", "opus", "wav", "aac", "wv", "strm",
];

async fn scan_addon(
    ctx: &AppContext,
    tmdb: &Option<sdks::RestClient<sdks::BearerAuth>>,
    addon: &Addon,
) -> Result<()> {
    let cfg = &addon.preset.config;
    let media_kind = cfg["media_kind"].as_str().unwrap_or("movie").to_string();

    let operator = build_operator(cfg, &addon.preset.kind)?;

    info!(addon = %addon.name, kind = %addon.preset.kind, media_kind, "opendal: scanning");

    let extensions: &[&str] = if media_kind == "track" {
        AUDIO_EXTENSIONS
    } else {
        VIDEO_EXTENSIONS
    };

    let track_num_re = Regex::new(r"^(\d{1,3})[.\s\-_\[\]]+").unwrap();

    let mut lister = operator.lister_with("/").recursive(true).await?;
    let mut seen_ids: Vec<Uuid> = Vec::new();
    let mut upserted = 0usize;

    while let Some(entry) = lister.try_next().await? {
        if entry.metadata().mode() != EntryMode::FILE {
            continue;
        }

        let path = entry.path().to_string();
        let name = entry.name().to_string();
        let ext = std::path::Path::new(&name)
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or("")
            .to_lowercase();

        if !extensions.contains(&ext.as_str()) {
            continue;
        }

        let row_id = common::get_stable_uuid(format!("{}:{}", addon.id, path));
        seen_ids.push(row_id);

        let stored_path: String = if ext == "strm" {
            match operator.read(&path).await {
                Ok(buf) => {
                    let url =
                        String::from_utf8_lossy(&buf.to_bytes()).trim().to_string();
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
                    .and_then(|m| m.as_str().parse::<i64>().ok());
                let clean_stem = if track_number.is_some() {
                    track_num_re.replace(&stem, "").into_owned()
                } else {
                    stem.clone()
                };
                let parsed = hunch::hunch(&clean_stem);
                let title = parsed.title().unwrap_or(clean_stem.as_str()).to_string();
                (Some(title), None, None, track_number, None, None)
            }
            "episode" => {
                let parsed = hunch::hunch(&stem);
                let season = parsed.season().map(|s| s as i64);
                let episode = parsed.episode().map(|e| e as i64);
                let year = parsed.year().map(|y| y as i64);
                let clean_title = parsed.title().unwrap_or(stem.as_str()).to_string();

                let existing_imdb = fetch_existing_imdb(ctx, addon.id, &path).await?;
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
                    } else {
                        jellyfin_ids.imdb.clone()
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
                let year = parsed.year().map(|y| y as i64);
                let clean_title = parsed.title().unwrap_or(stem.as_str()).to_string();

                let existing_imdb = fetch_existing_imdb(ctx, addon.id, &path).await?;
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
                    } else {
                        jellyfin_ids.imdb.clone()
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

        let size = Some(entry.metadata().content_length() as i64);
        let now = Utc::now().naive_utc().to_string();

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

    let deleted = prune_stale_paths(ctx, addon.id, &seen_ids).await?;

    info!(
        addon = %addon.name,
        upserted,
        deleted,
        "opendal: scan complete"
    );

    Ok(())
}

fn build_operator(
    cfg: &serde_json::Value,
    preset_kind: &str,
) -> Result<opendal::Operator> {
    match preset_kind {
        "opendal-webdav" => {
            let endpoint = cfg["endpoint"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("opendal-webdav: endpoint required"))?;
            let mut builder = opendal::services::Webdav::default().endpoint(endpoint);
            if let Some(u) = cfg["username"].as_str().filter(|s| !s.is_empty()) {
                builder = builder.username(u);
            }
            if let Some(p) = cfg["password"].as_str().filter(|s| !s.is_empty()) {
                builder = builder.password(p);
            }
            Ok(opendal::Operator::new(builder)?.finish())
        }
        "opendal-local" => {
            let path = cfg["path"]
                .as_str()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| anyhow::anyhow!("opendal-local: path required"))?;
            Ok(
                opendal::Operator::new(opendal::services::Fs::default().root(path))?
                    .finish(),
            )
        }
        other => anyhow::bail!("opendal: unknown preset kind {:?}", other),
    }
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
        let tmdb_id = resp.results.into_iter().next()?.id;

        let series = client
            .execute(
                sdks::tmdb::SeriesEndpoint::new(tmdb_id)
                    .with_cache(Duration::from_secs(86400)),
            )
            .await
            .ok()?;

        series.external_ids.as_ref().and_then(|e| e.imdb_id.clone())
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
        let tmdb_id = resp.results.into_iter().next()?.id;

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

    let mut tx = ctx.db.begin().await?;
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
        qb.build().execute(&mut *tx).await?;
    }

    let result = sqlx::query(
        "DELETE FROM opendal_files \
         WHERE addon_id = ? AND id NOT IN (SELECT id FROM _opendal_seen)",
    )
    .bind(addon_id)
    .execute(&mut *tx)
    .await?;

    tx.commit().await?;
    Ok(result.rows_affected() as usize)
}

#[cfg(test)]
mod tests {
    use regex::Regex;

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
                parsed.title().unwrap_or(""),
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
        ];

        for c in &cases {
            let parsed = hunch::hunch(c.stem);
            assert_eq!(
                parsed.title().unwrap_or(""),
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
                .and_then(|m| m.as_str().parse::<i64>().ok());

            let remainder = if track_number.is_some() {
                re.replace(c.stem, "").into_owned()
            } else {
                c.stem.to_string()
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
}
