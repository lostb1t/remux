use async_trait::async_trait;
use anyhow::{Result};
use dioxus_logger::tracing::info;
use serde::{Deserialize, Serialize};
use bon::Builder;
use bon::bon;
use bon::builder;
use std::sync::Arc;

use crate::sdks::core::endpoint::Endpoint;

use crate::sdks::jellyfin::{self, AuthenticationResult};
use crate::sdks::core::RestClient;


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Catalog {
    pub id: String,
    pub name: String,
}

#[async_trait(?Send)]
pub trait Server {
    fn host(&self) -> String;
    fn id(&self) -> String;
   // fn name(&self) -> String;
    //fn poster_url(&self, media: &Media) -> String;
    async fn connect(&mut self) -> Result<()>;
    async fn get_catalogs(&self) -> Result<Vec<Catalog>>;
}

#[derive(Clone)]
pub struct Jellyfin {
    pub host: String,
    pub username: String,
    pub password: String,
    pub id: String,
   // pub name: String,
    pub access_token: String,
    pub client: RestClient,
    //pub user_id: String,
}

//#[bon::buildable]

impl PartialEq for Jellyfin {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
    }
}

#[async_trait(?Send)]
impl Server for Jellyfin {
    fn host(&self) -> String {
        self.host.clone()
    }

    fn id(&self) -> String {
        self.id.clone()
    }

    //fn name(&self) -> String {
    //    self.name.clone()
   // }

   // fn poster_url(&self, media: &Media) -> String {
   //     format!("{}{}", self.host, media.poster_path)
   // }

    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Jellyfin...");
        //let (auth_token, user_id) = Self::authenticate(&self.host, &self.username, &self.password).await?;
        //self.client = Self::create_client(&self.host, &auth_token, &user_id)?;
        //self.auth_token = auth_token;
        //self.user_id = user_id;
        Ok(())
    }

    async fn get_catalogs(&self) -> Result<Vec<Catalog>> {
        Ok(vec![]) // Placeholder
    }
}

#[bon]
impl Jellyfin {
    #[builder]
    pub async fn new(host: String, username: String, password: String) -> Result<Jellyfin> {
      
       // let name = self.name().ok_or_else(|| eyre!("missing name"))?;
        let res = Jellyfin::authenticate(&host, &username, &password).await?;
        let access_token = res.access_token.expect("Expect access token");
        let id = res.server_id.expect("Expect access token");
        let client = Jellyfin::create_client(&host, &access_token, &res.user.unwrap().id.unwrap())?;
        
        Ok(Jellyfin {
            host,
            username,
            password,
            id,
          //  name: res.name,
            access_token,
            client,
           // user_id,
        })
    }

    fn anon_auth_header() -> &'static str {
        "Emby Client=\"Remux\", Device=\"Samsung Galaxy SIII\", DeviceId=\"xxx\", Version=\"1.0.0.0\""
    }

    async fn authenticate(host: &str, username: &str, password: &str) -> Result<AuthenticationResult> {
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