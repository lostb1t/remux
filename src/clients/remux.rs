use serde::{Deserialize, Serialize};

// use eyre::Result;
use reqwest;

pub static BASE_API_URL: &str = "http://100.95.68.69:8000/api/v1";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Media {
    pub id: u64,
    pub name: String,
}

#[derive(
    Copy,
    Serialize,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Deserialize,
    Hash,
    strum_macros::Display,
    strum_macros::EnumString,
    strum_macros::EnumIter
)]
#[serde(rename_all = "lowercase")]
pub enum ServiceType {
    Fs,
    Webdav,
    Ftp,
}

impl Default for ServiceType {
    fn default() -> Self {
        Self::Fs
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: u64,
    pub name: String,
    pub enable: bool,
    pub service_type: ServiceType,
    // pub config: ServiceType,
}

pub async fn get_media() -> Result<Vec<Media>, reqwest::Error> {
    let url = format!("{}/{}", BASE_API_URL, "media/file");
    let items = reqwest::get(&url).await?.json::<Vec<Media>>().await?;
    Ok(items)
}

pub async fn get_sources() -> Result<Vec<Source>, reqwest::Error> {
    let url = format!("{}/{}", BASE_API_URL, "media/source");
    let items = reqwest::get(&url).await?.json::<Vec<Source>>().await?;
    Ok(items)
}

pub async fn update_source(source: Source) -> Result<Source, reqwest::Error> {
    let client = reqwest::Client::new();
    let url = format!("{}/{}/{}", BASE_API_URL, "media/source", source.id);
    let items = client
        .patch(&url)
        .json(&source)
        .send()
        .await?
        .json::<Source>()
        .await?;
    Ok(items)
}
