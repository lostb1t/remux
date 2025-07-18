use crate::sdks::core::endpoint::Endpoint;
use anyhow::anyhow;
use anyhow::Result;
use async_trait::async_trait;
use bon::bon;
use bon::builder;
use bon::Builder;
//use derive_builder;
use dioxus::prelude::*;
use dioxus_logger::tracing;
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

#[derive(Builder, Serialize, Deserialize, Debug, Clone)]
pub struct Catalog {
    pub id: String,
    pub title: String,
}

impl TryFrom<sdks::jellyfin::BaseItemDto> for Catalog {
    type Error = anyhow::Error;

    fn try_from(item: sdks::jellyfin::BaseItemDto) -> anyhow::Result<Self, Self::Error> {
        Ok(Catalog::builder()
            .id(item.id.unwrap().to_string())
            .title(item.name.unwrap())
            .build())
    }
}

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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ConnectionStatus {
    //Connecting,
    Success,
    Failed,
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
}

#[derive(Serialize, PartialEq, Deserialize, Clone, Debug)]
pub struct ServerConfig {
    pub kind: ServerKind,
    pub host: String,
    pub username: String,
    pub password: String,
}

impl ServerConfig {
    // pub fn into_server(self) -> Arc<dyn Server> {
    //     match self.kind {
    //         ServerKind::Jellyfin => Arc::new(JellyfinServer::from_config(self)) as Arc<dyn Server>,
    //     }
    // }
    pub fn into_server(self) -> Box<dyn Server> {
        match self.kind {
            ServerKind::Jellyfin => Box::new(JellyfinServer::from_config(self)) as Box<dyn Server>,
        }
    }
}

#[async_trait(?Send)]
pub trait Server: Debug {
    fn host(&self) -> String;
    //  fn id(&self) -> String;
    fn status(&self) -> ConnectionStatus;
    async fn is_watched(&self, val: bool, media_item: &media::Media) -> Result<()>;
    async fn is_favorite(&self, val: bool, media_item: &media::Media) -> Result<()>;
    fn user_id(&self) -> Option<String>;
    fn into_config(&self) -> ServerConfig;
    // fn name(&self) -> String;
    fn image_url(&self, media_item: &media::Media, image_type: media::ImageType) -> String;
    async fn connect(&mut self) -> Result<()>;
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
    // series only
    async fn nextup(&self, item: &media::Media) -> Result<Vec<media::Media>>;
    async fn get_media_details(&self, id: String) -> Result<Option<media::Media>>;
}

#[derive(Clone, Debug)]
pub struct JellyfinServer {
    pub host: String,
    pub username: String,
    #[debug(skip)]
    pub password: String,
    //  pub id: String,
    // pub name: String,
    #[debug(skip)]
    pub access_token: Option<String>,

    // #[serde(skip, default)]
    #[debug(skip)]
    pub client: RestClient,
    //#[serde(skip, default)]
    pub status: ConnectionStatus,
    pub user_id: Option<String>,
}

//#[bon::buildable]

impl PartialEq for JellyfinServer {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
    }
}

#[async_trait(?Send)]
impl Server for JellyfinServer {
    fn host(&self) -> String {
        self.host.clone()
    }

    // fn id(&self) -> String {
    //     self.id.clone()
    //}

    fn status(&self) -> ConnectionStatus {
        self.status
    }

    fn user_id(&self) -> Option<String> {
        self.user_id.clone()
    }

    fn into_config(&self) -> ServerConfig {
        ServerConfig {
            kind: ServerKind::Jellyfin,
            host: self.host.clone(),
            username: self.username.clone(),
            password: self.password.clone(),
            //status: ConnectionStatus::Unknown,
        }
    }

    fn image_url(&self, media_item: &media::Media, image_type: media::ImageType) -> String {
        // debug!("IMAGE URL: {:?}", media_item);
        // debug!("IMAGE type: {:?}", image_type);
        //  let tag = match image_type {
        //     media::ImageType::Poster => media_item.poster.as_deref(),
        //     media::ImageType::Backdrop => media_item.backdrop.as_deref(),
        //     media::ImageType::Logo => media_item.logo.as_deref(),

        //};

        let it = match image_type {
            media::ImageType::Poster => "Primary",
            media::ImageType::Backdrop => "Backdrop",
            media::ImageType::Logo => "Logo",
            media::ImageType::Thumb => "Thumb",
        };

        format!(
            "{}/Items/{}/Images/{}",
            self.host,
            media_item.id,
            it.to_string(),
            // tag
        )
    }

    //fn name(&self) -> String {
    //    self.name.clone()
    // }

    // fn poster_url(&self, media: &Media) -> String {
    //     format!("{}{}", self.host, media.poster_path)
    // }

    async fn connect(&mut self) -> Result<()> {
        debug!("connecting to jellyfin");
        let res = Self::authenticate(&self.host, &self.username, &self.password).await?;
        let access_token = res.access_token.ok_or_else(|| anyhow!("Missing token"))?;
        let user_id = res
            .user
            .ok_or_else(|| anyhow!("Missing user"))?
            .id
            .ok_or_else(|| anyhow!("Missing user ID"))?;
        self.client = Self::create_client(&self.host, &access_token, &user_id)?;
        self.access_token = Some(access_token);
        self.user_id = Some(user_id);
        self.status = ConnectionStatus::Success;
        Ok(())
    }

    async fn is_watched(&self, val: bool, media_item: &media::Media) -> Result<()> {
        let endpoint = sdks::jellyfin::TogglePlayedEndpoint::builder()
            .user_id(self.user_id.clone().unwrap())
            .item_id(media_item.clone().id)
            .is_played(val)
            .build();

        let res = endpoint.query(&self.client).await?;
        debug!("{:?}", res);
        Ok(())
    }

    async fn is_favorite(&self, val: bool, media_item: &media::Media) -> Result<()> {
        let endpoint = sdks::jellyfin::ToggleFavoriteEndpoint::builder()
            .user_id(self.user_id.clone().unwrap())
            .item_id(media_item.clone().id)
            .is_favorite(val)
            .build();

        let res = endpoint.query(&self.client).await?;
        debug!("{:?}", res);
        Ok(())
    }

    #[tracing::instrument()]
    async fn get_stream_url(
        &self,
        item: media::Media,
        source: Option<media::MediaSource>,
        cap: capabilities::Capabilities,
    ) -> Result<String> {
        debug!("getstreamurl");
        //info!("{:?}",cap.to_device_profile() );
        let endpoint = sdks::jellyfin::PlaybackInfo::builder()
            .item_id(item.id.clone())
            .device_profile(cap.to_device_profile())
            .enable_all_subtitles(true)
            .maybe_media_source_id(source.clone().map(|p| p.id))
            .build();

        let res = endpoint.query(&self.client).await?;

        if let Some(url) = res.media_sources.first().unwrap().transcoding_url.clone() {
            // info!("{:?}",url );
            Ok(format!("{}{}", self.host, url))
        } else {
            let e = sdks::jellyfin::VideoStreamRequest::builder()
                .item_id(item.id.clone())
                .api_key(self.access_token.clone().expect("missing api key"))
                //.media_source_id(item.id)
                //.transcoding_protocol("hls".to_string())
                //.transcoding_container("ts".to_string())
                .static_(true)
                .build();

            Ok(format!(
                "{}{}.m3u8?{}",
                self.host,
                e.endpoint(),
                e.parameters().to_query_string()
            ))
        }
    }

    async fn get_media_sources(&self, item: media::Media) -> Result<Vec<media::MediaSource>> {
        Ok(vec![])
    }

    #[tracing::instrument()]
    async fn get_genres(&self) -> Result<Vec<media::Genre>> {
        let endpoint = sdks::jellyfin::ItemsFiltersEndpoint::builder()
            .include_item_types(vec![
                sdks::jellyfin::ItemType::Series,
                sdks::jellyfin::ItemType::Movie,
            ])
            // without is 10x slower....
            .maybe_user_id(self.user_id.clone())
            .build();

        let res = endpoint.query(&self.client).await?;
        //info!("TopNavbar items: {:?}", &res);

        let genres = res
            .genres
            .unwrap_or_default()
            .into_iter()
            .map(|s| media::Genre {
                id: s.clone(),
                name: s,
            })
            .collect();

        Ok(genres)
    }

    #[tracing::instrument(level = "debug")]
    async fn get_media_details(&self, id: String) -> Result<Option<media::Media>> {
        let endpoint = sdks::jellyfin::ItemsEndpoint::builder()
            .include_item_types(vec![])
            //.fields(vec!["premiere_date".to_string(), "genres".to_string(), "overview".to_string()])
            .ids(vec![id])
            .build();

        let res = endpoint.query(&self.client).await?;
        match res.items.first() {
            Some(item) => Ok(Some(item.to_owned().try_into()?)),
            None => Ok(None),
        }
    }

    #[tracing::instrument()]
    async fn get_media(&self, q: &MediaQuery) -> Result<Vec<media::Media>> {
        let endpoint =
            sdks::jellyfin::ItemsEndpoint::builder()
                .include_item_types(q.types.clone().try_into_vec().unwrap())
                .maybe_parent_id({
                    match &q.for_catalog {
                        Some(catalog)
                            if !["latest", "favorites", "continue_watching"]
                                .contains(&catalog.id.as_str()) =>
                        {
                            Some(catalog.id.clone())
                        }
                        _ => q.parent.clone().map(|p| p.id),
                    }
                })
                .limit(q.limit.clone())
                .start_index(q.offset.clone())
                .maybe_genres(
                    q.genres
                        .clone()
                        .map(|v| v.into_iter().map(|g| g.name).collect()),
                )
                .maybe_search_term(q.search_query.as_ref().map(|s| s.replace(' ', "+")))
                .maybe_filters(q.for_catalog.as_ref().and_then(
                    |catalog| match catalog.id.as_str() {
                        "favorites" => Some(vec![sdks::jellyfin::ItemFilter::IsFavorite]),
                        "continue_watching" => Some(vec![sdks::jellyfin::ItemFilter::IsResumable]),
                        _ => None,
                    },
                ))
                .maybe_sort_by(q.for_catalog.as_ref().and_then(
                    |catalog| match catalog.id.as_str() {
                        "latest" => Some(sdks::jellyfin::ItemSortBy::DateCreated),
                        _ => None,
                    },
                ))
                .build();
        //debug!("endpojnt: {:?}", endpoint);

        let res = endpoint.query(&self.client).await?;
        res.items.into_iter().map(|s| s.try_into()).collect()
    }

    #[tracing::instrument()]
    async fn nextup(&self, item: &media::Media) -> Result<Vec<media::Media>> {
        debug!("READING NEXTUP");
        let endpoint = sdks::jellyfin::NextUpEndpoint::builder()
            .user_id(self.user_id.clone().unwrap())
            .series_id(item.id.clone())
            .limit(1)
            .build();

        let res = endpoint.query(&self.client).await?;
        res.items.into_iter().map(|s| s.try_into()).collect()
    }

    // todo: rename to catogs
    #[tracing::instrument(level = "debug")]
    async fn get_catalogs(&self) -> Result<Vec<media::Media>> {
        let endpoint = sdks::jellyfin::ItemsEndpoint::builder()
            .include_item_types(vec![sdks::jellyfin::ItemType::BoxSet])
            //  .recursive(false)
            .limit(100)
            .build();

        let res = endpoint.query(&self.client).await?;
        let mut catalogs: Vec<media::Media> = res
            .items
            .into_iter()
            .filter_map(|s| s.try_into().ok())
            .collect();
        catalogs.push(media::Media {
            id: "latest".to_string(),
            title: "Latest".to_string(),
            media_type: media::MediaType::Catalog,
            ..Default::default()
        });

        catalogs.push(media::Media {
            id: "favorites".to_string(),
            title: "Favorites".to_string(),
            media_type: media::MediaType::Catalog,
            ..Default::default()
        });
        catalogs.insert(
            1,
            media::Media {
                id: "continue_watching".to_string(),
                title: "Continue Watching".to_string(),
                media_type: media::MediaType::Catalog,
                card_variant: components::CardVariant::Landscape,
                ..Default::default()
            },
        );
        Ok(catalogs)
    }
}

//#[bon]
impl JellyfinServer {
    // #[builder]
    pub fn new(host: String, username: String, password: String) -> JellyfinServer {
        // let name = self.name().ok_or_else(|| eyre!("missing name"))?;
        //let res = JellyfinServer::authenticate(&host, &username, &password).await?;
        //let access_token = res.access_token.expect("Expect access token");
        //let id = res.server_id.expect("Expect access token");
        let client = JellyfinServer::create_client(&host, "", "").unwrap();

        JellyfinServer {
            host,
            username,
            password,
            //  id,
            client,
            //  name: res.name,
            access_token: None,
            //     client,
            status: ConnectionStatus::default(),
            user_id: None,
        }
    }

    pub fn from_config(config: ServerConfig) -> Self {
        Self::new(config.host, config.username, config.password)
    }

    fn anon_auth_header() -> &'static str {
        "Emby Client=\"Remux\", Device=\"Samsung Galaxy SIII\", DeviceId=\"xxx\", Version=\"1.0.0.0\""
    }

    async fn authenticate(
        host: &str,
        username: &str,
        password: &str,
    ) -> Result<AuthenticationResult> {
        let client = RestClient::new(host)?.header("Authorization", Self::anon_auth_header());

        let endpoint = jellyfin::AuthenticateUserByName::builder()
            .username(username.to_string())
            .password(password.to_string())
            .build();

        Ok(endpoint.query(&client).await?)
    }

    fn create_client(host: &str, token: &str, user_id: &str) -> Result<RestClient> {
        let auth_header = format!(
            "Emby UserId=\"{}\", Token=\"{}\", Client=\"Android\", Device=\"Samsung Galaxy SIII\", DeviceId=\"xxx\", Version=\"1.0.0.0\"",
            user_id, token
        );
        Ok(RestClient::new(host)?.header("Authorization", &auth_header))
    }

    pub async fn reconnect(&mut self) -> Result<()> {
        self.connect().await
    }
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
    // map_error = r#"|e| e.to_string()"#
)]
pub async fn get_catalogs_cached(
    server: Arc<dyn crate::server::Server>,
) -> Result<Vec<media::Media>, ApiError> {
    Ok(server.get_catalogs().await?)
}

#[cached(
    ty = "TimedCache<String, Vec<media::Media>>",
    create = "{ TimedCache::with_lifespan(360) }",
    convert = r#"{ format!("media-{:?}-{:?}", server.user_id(), query.clone()) }"#,
    result = true
    // map_error = r#"|e| e.to_string()"#
)]
pub async fn get_media_cached(
    server: Arc<dyn crate::server::Server>,
    query: &MediaQuery,
) -> Result<Vec<media::Media>, ApiError> {
    //debug!("Fetching  for user: {:?}", server.user_id());
    Ok(server.get_media(query).await?)
}
