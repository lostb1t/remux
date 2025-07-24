use crate::sdks::core::endpoint::Endpoint;
use crate::server::JellyfinServer;
use crate::server::StremioServer;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use bon::bon;
use bon::builder;
use bon::Builder;
//use derive_builder;
use dioxus::prelude::*;
//use dioxus_logger::tracing;
use dioxus_logger::tracing::{debug, info};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing_subscriber::field::debug;
//use derive_more::Debug;
use crate::capabilities;
use crate::components;
use crate::media;
use crate::sdks;
use crate::sdks::core::ApiError;
use crate::sdks::core::RestClient;
use crate::sdks::jellyfin::{self, AuthenticationResult};
use crate::utils::TryIntoVec;
use derive_more::with_trait::Debug;

#[derive(Builder, Debug, Hash, Clone, PartialEq)]
#[builder(derive(Clone))]
pub struct MediaQuery {
    #[builder(default = 25)]
    pub limit: u32,

    #[builder(default = 0)]
    pub offset: u32,

    #[builder(default = vec![media::MediaType::Movie, media::MediaType::Series])]
    pub types: Vec<media::MediaType>,

    //pub catalog_id: Option<String>,
    //pub season_id: Option<String>,
    pub search_query: Option<String>,

    pub parent: Option<media::Media>,
    pub for_catalog: Option<media::Media>,
    pub genres: Option<Vec<media::Genre>>,
    //pub is_favorite: Option<bool>,
}

impl MediaQuery {
    pub fn key(&self) -> String {
        format!("{:?}", self.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Success,
    Unauthorized,
    Unreachable,
    Timeout,
    Unknown,
}

impl Default for ConnectionStatus {
    fn default() -> Self {
        ConnectionStatus::Unknown
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
#[serde(tag = "type")]
pub enum ServerKind {
    Jellyfin,
    Stremio,
}

use delegate::delegate;

#[derive(Debug, Clone)]
pub enum ServerInstance {
    Jellyfin(super::JellyfinServer),
    Stremio(super::StremioServer),
}

impl ServerInstance {
    pub fn from_config(config: ServerConfig) -> Result<Self> {
        let instance = match config.kind {
            ServerKind::Jellyfin => ServerInstance::Jellyfin(JellyfinServer::from_config(config)?),
            ServerKind::Stremio => ServerInstance::Stremio(StremioServer::from_config(config)?),
        };
        Ok(instance)
    }

    pub async fn from_credentials(
        kind: ServerKind,
        host: String,
        username: String,
        password: String,
    ) -> Result<Self> {
        match kind {
            ServerKind::Jellyfin => Ok(ServerInstance::Jellyfin(
                JellyfinServer::from_credentials(host, username, password).await?,
            )),
            ServerKind::Stremio => Ok(ServerInstance::Stremio(
                StremioServer::from_credentials(host, username, password).await?,
            )),
        }
    }
}

impl ServerInstance {
    delegate! {
        to match self {
            ServerInstance::Jellyfin(inner) => inner,
            ServerInstance::Stremio(inner) => inner,
        } {

        pub fn host(&self) -> String;
        pub async fn check_status(&self) -> Result<ConnectionStatus>;

        pub fn user_id(&self) -> Option<String>;
        pub fn into_config(&self) -> ServerConfig;
        pub fn image_url(&self, media_item: &media::Media, image_type: media::ImageType) -> Option<String>;

       // pub async fn authenticate(host: String, username: String, password: String) -> Result<AuthenticateResult>;
       // pub async fn from_username_and_password(&mut self) -> Result<()>;
        pub async fn is_watched(&self, val: bool, media_item: &media::Media) -> Result<()>;
        pub async fn is_favorite(&self, val: bool, media_item: &media::Media) -> Result<()>;
        pub async fn get_stream_url(&self, item: media::Media, source: Option<media::MediaSource>, cap: capabilities::Capabilities) -> Result<String>;
        pub async fn get_media_sources(&self, item: media::Media) -> Result<Vec<media::MediaSource>>;
        pub async fn get_catalogs(&self) -> Result<Vec<media::Media>>;
        pub async fn get_genres(&self) -> Result<Vec<media::Genre>>;
        pub async fn get_media(&self, q: &MediaQuery) -> Result<Vec<media::Media>>;
        pub async fn nextup(&self, item: &media::Media) -> Result<Vec<media::Media>>;
        pub async fn get_media_details(&self, id: String) -> Result<Option<media::Media>>;
        }
    }
}

#[derive(Serialize, PartialEq, Deserialize, Clone, Debug)]
pub struct ServerConfig {
    pub kind: ServerKind,
    pub host: String,
    pub username: String,

    // Set after authentication
    pub token: Option<String>,
    pub user_id: Option<String>,
}

impl ServerConfig {
    pub fn into_server(self) -> Result<ServerInstance> {
        let instance = match self.kind {
            ServerKind::Jellyfin => {
                ServerInstance::Jellyfin(super::JellyfinServer::from_config(self)?)
            }
            ServerKind::Stremio => {
                ServerInstance::Stremio(super::StremioServer::from_config(self)?)
            }
        };
        Ok(instance)
    }
}

#[derive(Serialize, PartialEq, Deserialize, Clone, Debug)]
pub struct AuthenticateResult {
    pub user_id: Option<String>,
    pub token: String,
}

#[async_trait(?Send)]
pub trait Server: Debug {
    fn host(&self) -> String;
    async fn check_status(&self) -> Result<ConnectionStatus>;
    fn user_id(&self) -> Option<String>;
    fn into_config(&self) -> ServerConfig;
    fn image_url(&self, media_item: &media::Media, image_type: media::ImageType) -> Option<String>;
    // async fn authenticate(host: String, username: String, password: String) -> Result<AuthenticateResult>;
    //async fn from_credentials(host: String, username: String, password: String) -> Result<dyn Server>;
    async fn is_watched(&self, val: bool, media_item: &media::Media) -> Result<()>;
    async fn is_favorite(&self, val: bool, media_item: &media::Media) -> Result<()>;
    async fn get_stream_url(
        &self,
        item: media::Media,
        source: Option<media::MediaSource>,
        cap: capabilities::Capabilities,
    ) -> Result<String>;
    async fn get_media_sources(&self, item: media::Media) -> Result<Vec<media::MediaSource>>;
    async fn get_catalogs(&self) -> Result<Vec<media::Media>>;
    async fn get_genres(&self) -> Result<Vec<media::Genre>>;
    async fn get_media(&self, q: &MediaQuery) -> Result<Vec<media::Media>>;
    async fn nextup(&self, item: &media::Media) -> Result<Vec<media::Media>>;
    async fn get_media_details(&self, id: String) -> Result<Option<media::Media>>;
}

#[derive(thiserror::Error, Debug)]
pub enum ServerError {
    #[error("Unauthorized (token expired?)")]
    Unauthorized,
    #[error("Network error: {0}")]
    Network(#[from] reqwest::Error),
    #[error("Internal error: {0}")]
    Internal(#[from] anyhow::Error),
    #[error("Other error: {0}")]
    Other(String),
}

impl From<ApiError> for ServerError {
    fn from(e: ApiError) -> Self {
        match e {
            ApiError::Unauthorized => ServerError::Unauthorized,
            other => ServerError::Other(other.to_string()), // <-- convert to String here
        }
    }
}

pub type ServerResult<T> = Result<T, ServerError>;

use cached::proc_macro::cached;
use cached::proc_macro::io_cached;
use cached::TimedCache;
use cached::*;

#[cached(
    ty = "TimedCache<String, Vec<media::Media>>",
    create = "{ TimedCache::with_lifespan(360) }",
    convert = r#"{ format!("collections-{:?}", server.user_id()) }"#,
    result = true
)]
pub async fn get_catalogs_cached(
    server: Arc<ServerInstance>,
) -> Result<Vec<media::Media>, ApiError> {
    Ok(server.get_catalogs().await?)
}

#[cached(
    ty = "TimedCache<String, Vec<media::Media>>",
    create = "{ TimedCache::with_lifespan(360) }",
    convert = r#"{ format!("media-{:?}-{:?}", server.user_id(), query.clone()) }"#,
    result = true
)]
pub async fn get_media_cached(
    server: Arc<ServerInstance>,
    query: &MediaQuery,
) -> Result<Vec<media::Media>, ApiError> {
    //debug!("Fetching  for user: {:?}", server.user_id());
    Ok(server.get_media(query).await?)
}
