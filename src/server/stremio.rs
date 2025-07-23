use crate::sdks::core::ApiError;
use crate::sdks::core::RestClient;
use crate::{
    capabilities::Capabilities,
    media::{self, Media, MediaSource},
    server::{ConnectionStatus, MediaQuery, Server, ServerConfig, ServerKind},
    APP_HOST,
};
use anyhow::{anyhow, Result};
use async_trait::async_trait;
use derive_more::with_trait::Debug;
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct Addon {
    // pub name: String,
    pub enabled: bool,
    pub url: String,

    #[debug(skip)]
    pub client: RestClient,
}

#[derive(Clone, Debug)]
pub struct StremioServer {
    pub host: String,
    pub status: ConnectionStatus,
    pub addons: Vec<Addon>,
}

impl StremioServer {
    pub fn new(host: String, username: String, password: String) -> Self {
        Self {
            status: ConnectionStatus::Success,
            host: host,
            addons: vec![Addon {
                client: RestClient::new("https://v3-cinemeta.strem.io").unwrap(),
                enabled: true,
                url: "https://v3-cinemeta.strem.io".to_string(),
            }],
        }
    }

    pub fn from_config(config: ServerConfig) -> Self {
        Self::new(config.host, config.username, config.password)
    }
}

#[async_trait(?Send)]
impl Server for StremioServer {
    fn host(&self) -> String {
        self.host.clone()
    }

    fn status(&self) -> ConnectionStatus {
        self.status
    }

    fn user_id(&self) -> Option<String> {
        None
    }

    fn into_config(&self) -> ServerConfig {
        ServerConfig {
            kind: ServerKind::Jellyfin, // You’ll want to add `Stremio` to `ServerKind`
            // host: self.host.clone(),
            host: "".to_string(),
            username: "".to_string(),
            password: "".to_string(),
        }
    }

    fn image_url(&self, media_item: &Media, image_type: media::ImageType) -> Option<String> {
        match image_type {
            media::ImageType::Poster => media_item.poster.clone(),
            media::ImageType::Backdrop => media_item.backdrop.clone(),
            media::ImageType::Logo => media_item.logo.clone(),
            media::ImageType::Thumb => media_item.thumb.clone(),
        }
    }

    async fn connect(&mut self) -> Result<()> {
        self.status = ConnectionStatus::Success;
        Ok(())
    }

    async fn is_watched(&self, _val: bool, _media_item: &Media) -> Result<()> {
        Ok(())
    }

    async fn is_favorite(&self, _val: bool, _media_item: &Media) -> Result<()> {
        Ok(())
    }

    async fn get_stream_url(
        &self,
        item: Media,
        _source: Option<MediaSource>,
        _cap: Capabilities,
    ) -> Result<String> {
        Ok("Nada".to_string())
        // Ok(item.url.ok_or_else(|| anyhow!("Missing stream URL"))?)
    }

    async fn get_media_sources(&self, item: media::Media) -> Result<Vec<media::MediaSource>> {
        Ok(vec![])
    }

    async fn get_catalogs(&self) -> Result<Vec<Media>> {
        // let endpoint = crate::sdks::stremio::ManifestEndpoint;
        // let manifest = self.client.query(&endpoint).await?;
        // let catalogs = manifest.catalogs.into_iter().map(|c| {
        //     media::Media::builder()
        //         .id(c.id.clone())
        //         .title(c.name)
        //         .media_type(c.kind.parse().unwrap_or(media::MediaType::Movie))
        //         .build()
        // }).collect::<Result<Vec<_>, _>>()?;
        // Ok(catalogs)
        Ok(vec![])
    }

    async fn get_genres(&self) -> Result<Vec<media::Genre>> {
        Ok(vec![]) // Depends on the catalog extras
    }

    async fn get_media(&self, q: &MediaQuery) -> Result<Vec<Media>> {
        // let catalog_id = q.for_catalog.as_ref().ok_or_else(|| anyhow!("Missing catalog"))?.id.clone();
        // let kind = q.types.first().unwrap_or(&media::MediaType::Movie).to_string();
        // let endpoint = crate::sdks::stremio::CatalogEndpoint::builder()
        //     .id(catalog_id)
        //     .kind(q.types.clone().try_into_vec().unwrap())
        //     .maybe_search(q.search_query.clone())
        //     // .genre(q.genres.as_ref().and_then(|g| g.first()).map(|g| g.name.clone()))
        //     .skip(q.offset)
        //     .build();
        // let res = self.client.query(&endpoint).await?;
        // let items = res.metas.into_iter().map(|m| {
        //     media::Media::builder()
        //         .id(m.id)
        //         .name(m.name)
        //         .kind(m.kind.parse().unwrap_or(media::MediaType::Movie))
        //         .poster(m.poster)
        //         .backdrop(m.background)
        //         .logo(m.logo)
        //         .build()
        // }).collect::<Result<Vec<_>, _>>()?;
        // Ok(items)
        Ok(vec![]) // Not applicable to Stremio
    }

    async fn nextup(&self, _item: &Media) -> Result<Vec<Media>> {
        Ok(vec![]) // Not applicable to Stremio
    }

    async fn get_media_details(&self, id: String) -> Result<Option<Media>> {
        // let kind = media::MediaType::Movie.to_string(); // fallback
        // let endpoint = crate::sdks::stremio::MetaEndpointBuilder::default()
        //     .id(id.clone())
        //     .kind(kind)
        //     .build()?;
        // let res = self.query.request(&endpoint).await?;
        // let meta = res.meta;
        // Ok(Some(media::Media::builder()
        //     .id(meta.id)
        //     .name(meta.name)
        //     .kind(meta.kind.parse().unwrap_or(media::MediaType::Movie))
        //     .poster(meta.poster)
        //     .backdrop(meta.background)
        //     .logo(meta.logo)
        //     .build()?))
        Ok(None)
    }
}
