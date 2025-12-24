use axum::http::Method;
use super::{Endpoint, ClientError,BasicAuth, RestClient };

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use serde_with::skip_serializing_none;
use std::time::Duration;
use chrono::{DateTime, Utc};
use bon::Builder;
use std::str::FromStr;
use http_cache_reqwest::{CacheMode};
use anyhow::Result;

#[derive(
    Default,
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
pub enum MediaType {
    #[strum(to_string = "movie")]
    Movie,
    #[strum(to_string = "series")]
    Series,
    #[strum(to_string = "tv")]
    Tv,
    #[strum(to_string = "events")]
    Events,
    #[default]
    Unknown,
}

#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "camelCase")]
pub enum ResourceType {
    #[strum(to_string = "stream")]
    Stream,
    #[strum(to_string = "subtitles")]
    Subtitles,
    #[strum(to_string = "catalog")]
    Catalog,
    #[strum(to_string = "meta")]
    Meta,
    #[strum(to_string = "addon_catalog")]
    AddonCatalog,
}

#[derive(Debug, Clone)]
pub struct ManifestEndpoint;

impl Endpoint for ManifestEndpoint {
    type Output = Manifest;

    fn path(&self) -> String {
        "/manifest.json".into()
    }
    
    fn cache_mode(&self) -> Option<CacheMode> {
        Some(CacheMode::ForceCache)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub resources: Vec<Resource>,
    pub types: Vec<String>,
    pub catalogs: Vec<Catalog>,
    pub id_prefixes: Option<Vec<String>>,
    pub logo: Option<String>,
}

impl Manifest {
  pub fn get_catalog_by_id(&self, id: &str) -> Option<Catalog> {
        self.catalogs.iter().find(|c| c.id == id).cloned()
  }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Resource {
    #[serde(deserialize_with = "deserialize_simple")]
    Simple(ResourceType),
    Detailed(ResourceRef),
}

fn deserialize_simple<'de, D>(d: D) -> Result<ResourceType, D::Error>
where
    D: serde::Deserializer<'de>,
{
    // Only accept a string for the Simple variant:
    let s = String::deserialize(d)?;
    ResourceType::from_str(&s).map_err(serde::de::Error::custom)
}

impl Resource {
    pub fn resource_type(&self) -> ResourceType {
        match self {
            Resource::Simple(s) => s.clone(),
            Resource::Detailed(r) => r.name.clone(),
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ResourceRef {
    pub name: ResourceType,
    pub types: Vec<String>,
    pub id_prefixes: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct Catalog {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: MediaType,
    pub name: String,
    #[serde(default)]
    pub extra: Vec<ExtraProp>,
}


impl Catalog {
    fn has_search(&self) -> bool {
        for extra in &self.extra {
            if extra.name == "search".to_string() {
                return true;
            }
        }
        false
    }

    
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct ExtraProp {
    pub name: String,
    // #[serde(rename = "isRequired")]
    //  pub is_required: bool,
    pub options: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEndpoint {
    #[serde(skip)]
    pub kind: MediaType,
    #[serde(skip)]
    pub id: String,

    pub search: Option<String>,
    pub genre: Option<String>,
    pub skip: Option<u32>,
    //pub extra: Option<HashMap<String, String>>,
}

impl Endpoint for CatalogEndpoint {
    type Output = CatalogResponse;

    fn path(&self) -> String {
        let mut ep = format!("/catalog/{}/{}", self.kind, self.id);

        let mut extras = Vec::new();
        if let Some(skip) = self.skip {
            extras.push(format!("skip={}", skip));
        }
        if let Some(search) = &self.search {
            extras.push(format!("search={}", search));
        }
        if let Some(genre) = &self.genre {
            extras.push(format!("genre={}", genre));
        }

        if !extras.is_empty() {
            ep.push('/');
            ep.push_str(&extras.join("&"));
        }

        ep.push_str(".json");
        ep
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct CatalogResponse {
    pub metas: Vec<Meta>,
}

// #[skip_serializing_none]
#[derive(Debug, Default, Clone, Builder)]
pub struct MetaEndpoint {
    pub media_type: MediaType,
    pub id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl Endpoint for MetaEndpoint {
    type Output = MetaResponse;

    fn path(&self) -> String {
        let mut id = self.id.clone();
        if self.season.is_some() || self.episode.is_some() {
            id = format!(
                "{}:{}:{}",
                id,
                self.season.unwrap_or(0),
                self.episode.unwrap_or(0)
            );
        }
        format!("/meta/{}/{}.json", self.media_type, id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct MetaResponse {
    pub meta: Meta,
}

/// TODO: Add filename for better matching
#[derive(Debug, Clone, Builder)]
pub struct SubtitlesEndpoint {
    pub media_type: MediaType,
    pub imdb_id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl Endpoint for SubtitlesEndpoint {
    type Output = SubtitlesResponse;

    fn path(&self) -> String {
        format!("/subtitles/{}/{}.json", self.media_type, self.imdb_id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct SubtitlesResponse {
   pub subtitles: Vec<Subtitle>,
}


#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subtitle {
    pub id: String,
    pub url: String,
    pub sub_encoding: Option<String>,
    pub lang: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    // #[serde(alias = "imdb_id", alias = "imdbId")]
    #[serde(rename = "imdb_id")]
    pub imdb_id: Option<String>,
    pub country: Option<String>,
    pub description: Option<String>,
    pub genre: Option<Vec<String>>,
    pub imdb_rating: Option<String>,
    pub name: Option<String>,
    pub released: Option<DateTime<Utc>>,
    pub slug: Option<String>,
    #[serde(rename = "type")]
    pub media_type: MediaType,
    //pub writer: Option<Vec<String>>,
    pub year: Option<String>,
    pub moviedb_id: Option<u64>,

    // pub popularities: Option<Popularities>,
    // pub trailers: Option<Vec<String>>,
    //pub cast: Option<Vec<String>>,
    //pub director: Option<Vec<String>>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub awards: Option<String>,
    pub popularity: Option<f64>,
    pub poster: Option<String>,
    pub id: String,
    pub genres: Option<Vec<String>>,
    pub release_info: Option<String>,

    #[serde(default, deserialize_with = "deserialize_opt_duration_empty_ok")]
    pub runtime: Option<Duration>,

    // #[serde(rename = "videos")]
    pub videos: Option<Vec<Episode>>,
    // pub trailer_streams: Option<Vec<String>>,
    // pub links: Option<Vec<Link>>,
    // pub behavior_hints: Option<BehaviorHints>,
  
  }

use serde::Deserializer;
use serde::de::Error as _;
//use std::time::Duration;

fn deserialize_opt_duration_empty_ok<'de, D>(
    de: D,
) -> Result<Option<Duration>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<String>::deserialize(de)?;
    match opt {
        None => Ok(None),
        Some(s) => {
            let t = s.trim();
            if t.is_empty() {
                Ok(None)
            } else {
                duration_str::parse(t).map(Some).map_err(D::Error::custom)
            }
        }
    }
}

impl Meta {
    pub fn get_season_numbers(&self) -> Vec<i32> {
        // dbg!(&self);
        if let Some(episodes) = self.videos.as_ref() {
            let mut seasons: Vec<i32> =
                episodes.iter().filter_map(|e| e.season).collect();
            seasons.sort_unstable();
            seasons.dedup();
            seasons
        } else {
            vec![]
        }
    }

    pub fn get_episode_by_id(&self, id: String) -> Option<&Episode> {
        if let Some(episodes) = &self.videos {
            episodes.into_iter().find(|e| e.id == id)
        } else {
            None
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: String,
    pub name: Option<String>,
    pub released: Option<String>,
    pub thumbnail: Option<String>,
    pub episode: Option<i32>,
    pub season: Option<i32>,
    pub overview: Option<String>,
    pub number: Option<i32>,
    pub description: Option<String>,
    pub rating: Option<String>,
    pub first_aired: Option<String>,
}



#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchQuery {
    #[serde(rename = "type")]
    pub kind: MediaType,
    pub id: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub success: bool,
    pub detail: Option<serde_json::Value>,
    pub data: SearchData,
    pub error: Option<serde_json::Value>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchData {
    pub filtered: i64,
    pub results: Vec<Stream>,
    pub errors: Vec<serde_json::Value>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub info_hash: String,
    pub url: Option<String>,
    pub nzb_url: Option<String>,
    pub rar_urls: Option<Vec<String>>,
    pub seven_zip_urls: Option<Vec<String>>,
    pub tar_urls: Option<Vec<String>>,
    pub tgz_urls: Option<Vec<String>>,
    pub seeders: Option<i64>,
    pub age: Option<i64>,
    pub sources: Option<Vec<String>>,
    pub yt_id: Option<String>,
    pub external_url: Option<String>,
    pub file_idx: Option<i64>,
    pub proxied: bool,
    pub filename: String,
    pub folder_name: Option<String>,
   // pub size: i64,
    //pub folder_size: Option<i64>,
    pub message: Option<String>,
    pub library: bool,
    pub addon: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub indexer: Option<String>,
    pub duration: i64,
    pub video_hash: Option<String>,
    pub subtitles: Vec<serde_json::Value>,
    pub country_whitelist: Vec<String>,
    pub request_headers: HashMap<String, String>,
    pub response_headers: HashMap<String, String>,
    pub parsed_file: ParsedFile,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl Stream {
   pub fn id(&self) -> String {
     self.info_hash.clone()
    }
    
    pub fn probe(&self) -> Result<super::jellyfin::MediaSourceInfo> {
        let id = self.id();

        // debug!("Probing: {}", self.url.clone().unwrap());
        let info = ffprobe::ffprobe(self.url.clone().unwrap())?;

        //dbg!(&info);
        let mut source: super::jellyfin::MediaSourceInfo = info.into();
        // source.id = Some(id.clone());
        // source.e_tag = Some(id.clone());

        // if include_external.as_ref().unwrap_or(&false) {

        // }

        Ok(source)
    }
  }

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ParsedFile {
    pub title: Option<String>,
    pub year: Option<String>,

    pub resolution: Option<String>,
    pub quality: Option<String>,
    pub encode: Option<String>,

    pub release_group: Option<String>,
    pub edition: Option<String>,

    pub remastered: Option<bool>,
    pub repack: Option<bool>,
    pub uncensored: Option<bool>,
    pub unrated: Option<bool>,
    pub upscaled: Option<bool>,

    pub container: Option<String>,
    pub extension: Option<String>,

    pub visual_tags: Vec<String>,
    pub audio_tags: Vec<String>,
    pub audio_channels: Vec<String>,

    pub languages: Vec<String>,

    pub season_pack: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Search {
    #[serde(rename = "type")]
    pub kind: MediaType,
    pub id: String,
    pub format: bool,
}

impl Endpoint for Search {
    type Output = SearchResponse;

    fn method(&self) -> Method {
        Method::GET
    }

    fn path(&self) -> String {
        "/search".to_string()
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut q = Vec::with_capacity(3);
        q.push(("type".to_string(), self.kind.clone().to_string()));
        q.push(("id".to_string(), self.id.clone()));
        q.push(("format".to_string(), if self.format { "true" } else { "false" }.to_string()));
        q
    }
}

pub fn search_client(base: &str, username: String, password: String) -> Result<RestClient<BasicAuth>, url::ParseError> {
    Ok(RestClient::new(base)?.with_auth(BasicAuth { username, password }))
}

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?)
}

pub async fn search(
    client: &RestClient<BasicAuth>,
    kind: impl Into<MediaType>,
    id: impl Into<String>,
) -> Result<SearchResponse, ClientError> {
    client
        .execute(&Search {
            kind: kind.into(),
            id: id.into(),
            format: true,
        })
        .await
}