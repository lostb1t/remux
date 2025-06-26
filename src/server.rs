use async_trait::async_trait;
use dioxus_logger::tracing::info;
use dyn_clone::DynClone;
use eyre::Result;
use jellyfin_api;
use serde::{Deserialize, Serialize, Serializer};
use std::sync::Arc;

// pub type Servers = Vec<Box<dyn Server>>;
//pub type Servers = Vec<Box<dyn Server>>;
pub type Servers = Vec<Jellyfin>;


#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Session {
    pub id: String,
    pub media: Option<Media>,
    pub user_name: String
}

impl PartialEq<Session> for Session {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct Media {
    //pub id: String,
    pub id: String,
    pub name: String,
    pub poster_path: String
}

impl From<jellyfin_api::types::SessionInfoDto> for Session {
    fn from(value: jellyfin_api::types::SessionInfoDto) -> Self {
        //info!("{:?}", value);
        Session {
            id: value.id.unwrap(),
            media: match value.now_playing_item {
              Some(v) => Some(v.into()),
              None => None
            },
            user_name: value.user_name.unwrap(),
        }
    }
}

impl From<jellyfin_api::types::BaseItemDto> for Media {
    fn from(value: jellyfin_api::types::BaseItemDto) -> Self {
        Media {
            id: format!("{}", value.id.unwrap()),
            name: value.name.unwrap(),
            poster_path: format!("/Items/{}/Images/Primary", value.id.unwrap())
        }
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Server: DynClone {
    fn host(&self) -> String;
    fn id(&self) -> String;
    fn name(&self) -> String;
    fn poster_url(&self, media: &Media) -> String;
    async fn connect(&mut self) -> Result<()>;
    async fn sessions(&self) -> Result<Vec<Session>>;
}

#[derive(Debug, Clone, Default)]
pub struct Jellyfin {
    pub host: String,
    pub username: String,
    pub password: String,
    pub id: String,
    pub name: String,
    pub auth_token: Option<String>,
    pub client: Arc<Option<jellyfin_api::Client>>,
}

impl PartialEq<Jellyfin> for Jellyfin {
    fn eq(&self, other: &Self) -> bool {
        self.host() == other.host()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Server for Jellyfin {
    fn host(&self) -> String {
        self.host.clone()
    }
    
    fn id(&self) -> String {
        self.id.clone()
    }
    
   fn name(&self) -> String {
        self.name.clone()
   }
   
   fn poster_url(&self, media: &Media) -> String {
      format!("{}{}", self.host, media.poster_path)
   }

    async fn connect(&mut self) -> Result<()> {
        info!("Connecting to Jellyfin");
        let mut headers = reqwest::header::HeaderMap::new();
        let auth_header = "MediaBrowser Client=\"Remux\", Device=\"device\", DeviceId=\"ZQ9YQHHrUzk24vV\", Version=\"10.10.5\"";
        headers.insert(reqwest::header::AUTHORIZATION, reqwest::header::HeaderValue::from_str(&auth_header).unwrap());
        let rclient = reqwest::ClientBuilder::new()
            .default_headers(headers) // Add this line to the generated code
            .build()
            .unwrap();
          let client =
          jellyfin_api::Client::new_with_client(self.host.as_str(), rclient);
        // let client = jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev");
        let result = client
            .authenticate_user_by_name()
            .body(
                jellyfin_api::types::AuthenticateUserByName::builder()
                    .pw(self.password.clone())
                    .username(self.username.clone()),
            )
            .send()
            .await;
        self.auth_token = Some(result.unwrap().into_inner().access_token.unwrap());
        // info!("{:?}", &result);
        // app.user.set(Some(result.unwrap().into_inner()));

        let mut headers = reqwest::header::HeaderMap::new();
        let auth_header = format!("MediaBrowser Client=\"Remux\", Device=\"device\", DeviceId=\"ZQ9YQHHrUzk24vV\", Version=\"10.10.5\", Token=\"{}\"", self.auth_token.clone().unwrap().as_str());
        headers.insert(reqwest::header::AUTHORIZATION, reqwest::header::HeaderValue::from_str(&auth_header).unwrap());
        let rclient = reqwest::ClientBuilder::new()
            .default_headers(headers) // Add this line to the generated code
            .build()
            .unwrap();
        //let client = jellyfin_api::Client::new("https://jellyfin.sjoerdarendsen.dev");
        let client =
            jellyfin_api::Client::new_with_client(self.host.as_str(), rclient);
        self.client = Some(client);
        Ok(())
    }

    async fn sessions(&self) -> Result<Vec<Session>> {
        let result = self.client.clone().unwrap().get_sessions().send().await;
        //   let result = spawn(async move {
        //     self.client.clone().unwrap().get_sessions().send().await
        // }).await?;
        // info!("{:?}", &result);
        Ok(convert_vec(result.unwrap().into_inner()))
        // app.user.set(Some(result.unwrap().into_inner()));

        // Ok(vec![Session {
        //     id: "1363626".to_string(),
        // }])
    }
}

fn convert_vec<T, U>(v: Vec<T>) -> Vec<U>
where
    T: Into<U>,
{
    v.into_iter().map(Into::into).collect()
}
