

//use crate::sdks::jellyfin;
//use axum::Json;
//use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use eyre::Result;
//use futures_util::future::join_all;
//use futures_util::future::try_join_all;
//use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest;
use reqwest::Client;
use reqwest::header;
//use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use serde::{Deserialize, Serialize};
use strum_macros;
use dioxus_logger::tracing::{debug, error, info};
use serde::Deserializer;
use std::str::FromStr;


#[derive(Default, strum_macros::Display, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    #[strum(to_string = "movie")]
    Movie,
    #[strum(to_string = "series")]
    Series,
    #[strum(to_string = "tv")]
    Tv,
    #[default]
    Unknown,
}

#[derive(strum_macros::Display, strum_macros::EnumString, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ResourceType {
    Stream,
    Subtitles,
    Catalog,
}

#[derive(Serialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
    //#[serde(default)]
    pub types: Option<Vec<MediaType>>,
    //#[serde(default)] 
    pub id_prefixes: Option<Vec<String>>,
    pub type_: ResourceType
}

impl<'de> Deserialize<'de> for Resource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[serde(remote = "Resource")]
        struct ResourceFull {
            name: String,
            type_: ResourceType,
            types: Option<Vec<MediaType>>,
            id_prefixes: Option<Vec<String>>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ResourceHelper {
            Simple(String),
            #[serde(with = "ResourceFull")]
            Full(Resource),
        }

        
        Ok(match ResourceHelper::deserialize(deserializer)? {
            ResourceHelper::Simple(name) => Resource {
                name: name.clone(),
                type_: ResourceType::from_str(&name).unwrap(), // auto-convert if needed
                types: None,
                id_prefixes: None,
            },
            ResourceHelper::Full(full) => full,
        })
    }
}


// Example code that deserializes and serializes the model.
// extern crate serde;
// #[macro_use]
// extern crate serde_derive;
// extern crate serde_json;
//
// use generated_module::[object Object];
//
// fn main() {
//     let json = r#"{"answer": 42}"#;
//     let model: [object Object] = serde_json::from_str(&json).unwrap();
// }

extern crate serde_derive;

#[derive(Serialize, Deserialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    pub id: String,
    version: String,
    name: String,
    description: Option<String>,
    catalogs: Option<Vec<Catalog>>,
    resources: Vec<Resource>,
    types: Vec<MediaType>,
    id_prefixes: Option<Vec<String>>,
    logo: Option<String>,
}

#[derive(strum_macros::Display, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreamNameField {
    Title,
    Name,
}

#[derive(Debug, Clone)]
pub struct Addon {
    pub url: String,
    pub manifest: Manifest,
    //pub stream_name_field: StreamNameField
}

impl Addon {
    pub async fn new(url: String, client: &Client) -> Result<Self> {
        let manifest: Manifest = client
            .get(format!("{}", url).as_str())
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(Self {
            url: url.replace("/manifest.json", ""),
            manifest,
        })
    }
    
    
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: MediaType,
    pub name: String,
    #[serde(rename = "metas")]
    pub items: Option<Vec<CatalogItem>>,

    // we need a global id
    //pub uuid: Some(String)
   // pub addon_manifest: Option<Manifest>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogItem {
    pub id: String,
    #[serde(rename = "type")]
    pub type_: MediaType,
    pub name: Option<String>,
    pub poster: Option<String>,
}
