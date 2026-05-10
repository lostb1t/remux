use anyhow::{Result, bail};
use async_trait::async_trait;
use std::pin::Pin;
use std::sync::Arc;
use uuid::Uuid;

use futures::Stream;
use remux_sdks::stremio::MediaType as StremioMediaType;

use super::{
    AddonKind, AddonMetadata, AddonOption, AddonOptionType, AddonPreset,
    AddonPresetRegistration, AddonSelectOption, CatalogInfo, MediaKind, ResourceType,
};
use crate::{AppContext, common, db};

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

impl OpendalAddon {
    fn stream_url(&self, path: &str) -> String {
        if path.contains("://") {
            return path.to_string();
        }
        match self.backend.as_str() {
            "local" => format!(
                "file://{}/{}",
                self.root.trim_end_matches('/'),
                path.trim_start_matches('/')
            ),
            _ => format!(
                "{}/{}",
                self.root.trim_end_matches('/'),
                path.trim_start_matches('/')
            ),
        }
    }
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
                db::Media {
                    id: stable_id,
                    title: f.name.clone(),
                    kind: db::MediaKind::Stream,
                    url: Some(self.stream_url(&f.path)),
                    parent_id: Some(media.id),
                    ..Default::default()
                }
            })
            .collect();

        Ok(streams)
    }
}
