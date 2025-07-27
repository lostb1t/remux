use crate::sdks::core::endpoint::Endpoint;
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
use super::{ConnectionStatus, MediaQuery, Server, ServerConfig, ServerKind};
use crate::capabilities;
use crate::components;
use crate::media;
use crate::sdks;
use crate::sdks::core::ApiError;
use crate::sdks::core::RestClient;
use crate::sdks::jellyfin::{self, AuthenticationResult};
use crate::settings;
use crate::utils::TryIntoVec;
use derive_more::with_trait::Debug;

#[derive(Clone, Debug)]
pub struct JellyfinServer {
    pub host: String,
    pub username: String,
    //  #[debug(skip)]
    //  pub password: String,
    //  pub id: String,
    // pub name: String,
    #[debug(skip)]
    pub access_token: String,

    // #[serde(skip, default)]
    #[debug(skip)]
    pub client: RestClient,
    //#[serde(skip, default)]
    // pub status: ConnectionStatus,
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

    async fn check_status(&self) -> Result<ConnectionStatus> {
        todo!("implement")
    }

    fn user_id(&self) -> Option<String> {
        self.user_id.clone()
    }

    fn into_config(&self) -> ServerConfig {
        ServerConfig {
            kind: ServerKind::Jellyfin,
            host: self.host.clone(),
            username: self.username.clone(),
            token: Some(self.access_token.clone()),
            user_id: self.user_id.clone(),
            //status: ConnectionStatus::Unknown,
        }
    }

    fn image_url(&self, media_item: &media::Media, image_type: media::ImageType) -> Option<String> {
        // jf doesnt have textless posters
        if image_type == media::ImageType::PosterTextless {
            return None;
        }

        let tag = match image_type {
            media::ImageType::Poster => media_item.poster.as_deref(),
            media::ImageType::Backdrop => media_item.backdrop.as_deref(),
            media::ImageType::Logo => media_item.logo.as_deref(),
            media::ImageType::Thumb => media_item.thumb.as_deref(),
            _ => None,
        };
        //debug!(?tag, "yo");
        if tag.is_none() {
            return None;
        }

        let it = match image_type {
            media::ImageType::Poster => "Primary",
            media::ImageType::Backdrop => "Backdrop",
            media::ImageType::Logo => "Logo",
            media::ImageType::Thumb => "Thumb",
            _ => return None,
        };

        Some(format!(
            "{}/Items/{}/Images/{}",
            self.host,
            media_item.id,
            it.to_string(),
            // tag
        ))
    }

    //fn name(&self) -> String {
    //    self.name.clone()
    // }

    // fn poster_url(&self, media: &Media) -> String {
    //     format!("{}{}", self.host, media.poster_path)
    // }
    //    async fn authenticate(
    //         host: String,
    //         username: String,
    //         password: String,
    //     ) -> Result<super::AuthenticateResult> {
    //         let client = RestClient::new(host)?.header("Authorization", &Self::anon_auth_header());

    //         let endpoint = jellyfin::AuthenticateUserByName::builder()
    //             .username(username.to_string())
    //             .password(password.to_string())
    //             .build();

    //         Ok(endpoint.query(&client).await?)
    //     }

    //

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

    async fn get_stream_url(
        &self,
        item: media::Media,
        source: Option<media::MediaSource>,
        cap: capabilities::Capabilities,
    ) -> Result<String> {
        //info!("{:?}",cap.to_device_profile() );
        let endpoint = sdks::jellyfin::PlaybackInfo::builder()
            .item_id(item.id.clone())
            .device_profile(cap.to_device_profile())
            .enable_all_subtitles(true)
           // .subtitle_stream_index(3)
            .maybe_media_source_id(source.clone().map(|p| p.id))
            .build();

        let res = endpoint.query(&self.client).await?;

        if let Some(url) = res.media_sources.first().unwrap().transcoding_url.clone() {
            // info!("{:?}",url );
            Ok(format!("{}{}&SubtitleMethod=Hls", self.host, url))
        } else {
            let e = sdks::jellyfin::VideoStreamRequest::builder()
                .item_id(item.id.clone())
                .api_key(self.access_token.clone())
                .subtitle_method("HLs".to_string())
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
        //debug!(?q, ?endpoint, "media");

        let res = endpoint.query(&self.client).await?;
        res.items.into_iter().map(|s| s.try_into()).collect()
    }

    async fn nextup(&self, item: &media::Media) -> Result<Vec<media::Media>> {
        let endpoint = sdks::jellyfin::NextUpEndpoint::builder()
            .user_id(self.user_id.clone().unwrap())
            .series_id(item.id.clone())
            .limit(1)
            .build();

        let res = endpoint.query(&self.client).await?;
        res.items.into_iter().map(|s| s.try_into()).collect()
    }

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
        catalogs.push(
            media::Media::builder()
                .id("latest".to_string())
                .title("Latest".to_string())
                .media_type(media::MediaType::Catalog)
                .build(),
        );
        catalogs.push(
            media::Media::builder()
                .id("favorites".to_string())
                .title("Favorites".to_string())
                .media_type(media::MediaType::Catalog)
                .build(),
        );
        catalogs.insert(
            1,
            media::Media::builder()
                .id("continue_watching".to_string())
                .title("Continue Watching".to_string())
                .media_type(media::MediaType::Catalog)
                .card_variant(settings::SettingField {
                    default: components::CardVariant::Landscape,
                    value: None,
                    locked: false,
                })
                .build(),
        );
        Ok(catalogs)
    }
}

//#[bon]
impl JellyfinServer {
    // #[builder]

    pub async fn from_credentials(
        host: String,
        username: String,
        password: String,
    ) -> Result<Self> {
        debug!("Connecting to jellyfin");
        let res = Self::authenticate(&host, &username, &password).await?;
        let access_token = res
            .access_token
            .ok_or_else(|| anyhow!("Missing access token"))?;
        let user_id = res
            .user
            .ok_or_else(|| anyhow!("Missing user"))?
            .id
            .ok_or_else(|| anyhow!("Missing user ID"))?;
        let client = Self::create_client(&host, &access_token, &user_id)?;

        Ok(Self {
            host,
            username,
            //  id,
            client,
            //  name: res.name,
            access_token: access_token,
            //     client,
            // status: ConnectionStatus::Success,
            user_id: Some(user_id),
        })
    }

    async fn authenticate(
        host: &str,
        username: &str,
        password: &str,
    ) -> Result<AuthenticationResult> {
        let client = RestClient::new(host)?.header("Authorization", &Self::anon_auth_header());

        let endpoint = jellyfin::AuthenticateUserByName::builder()
            .username(username.to_string())
            .password(password.to_string())
            .build();

        Ok(endpoint.query(&client).await?)
    }

    pub fn from_config(config: ServerConfig) -> Result<Self> {
        let token = config
            .token
            .clone()
            .ok_or_else(|| anyhow!("Missing token"))?;
        let user_id = config
            .user_id
            .clone()
            .ok_or_else(|| anyhow!("Missing user id"))?;
        let client = Self::create_client(&config.host, &token, &user_id)?;
        Ok(Self {
            host: config.host,
            username: config.username,
            access_token: token,
            client,
            user_id: config.user_id,
            // status: ConnectionStatus::Success,
        })
    }

    fn anon_auth_header() -> String {
        let app = crate::APP_HOST.peek();
        format!(
            "Emby Client=\"Remux\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            app.device_name, app.device_id, app.remux_version
        )
    }

    fn create_client(host: &str, token: &str, user_id: &str) -> Result<RestClient> {
        let app = crate::APP_HOST.peek();
        let auth_header = format!(
            "Emby UserId=\"{}\", Token=\"{}\", Client=\"Remux\", Device=\"{}\", DeviceId=\"{}\", Version=\"{}\"",
            user_id,
            token,
            app.device_name,
            app.device_id,
            app.remux_version
        );
        Ok(RestClient::new(host)?.header("Authorization", &auth_header))
    }

    //pub async fn reconnect(&mut self) -> Result<()> {
    //    self.connect().await
    //}
}
