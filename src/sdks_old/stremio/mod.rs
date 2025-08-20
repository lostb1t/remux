use crate::sdks::jellyfin;
use axum::Json;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use eyre::Result;
use futures_util::future::join_all;
use futures_util::future::try_join_all;
use http_cache_reqwest::{CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions};
use reqwest;
use reqwest::Client;
use reqwest::header;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use serde::{Deserialize, Serialize};
use strum_macros;
use tracing::{debug, error, info};
use serde::Deserializer;
use std::str::FromStr;
use tracing::instrument;
use serde_json::Value;

#[derive(Default, strum_macros::EnumString, strum_macros::Display, Debug, Clone, PartialEq, Serialize, Deserialize)]
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

#[derive(strum_macros::Display, strum_macros::EnumString, Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    AddonCatalog
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

#[derive(Serialize, PartialEq, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Resource {
    pub name: String,
   // #[serde(default)]
    pub types: Option<Vec<MediaType>>,
   // #[serde(default)] 
    pub id_prefixes: Option<Vec<String>>,
    pub type_: ResourceType
}


impl<'de> Deserialize<'de> for Resource {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
    //  let raw: Value = Deserialize::deserialize(deserializer)?;

        // Print the raw JSON
       // println!("Raw JSON: {}", raw);
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct ResourceFull {
            name: String,
            //type_: ResourceType,
            //types: Option<Vec<MediaType>>,
            id_prefixes: Option<Vec<String>>,
        }

        #[derive(Deserialize)]
        #[serde(untagged)]
        enum ResourceHelper {
            Simple(String),
            Full(ResourceFull),
        }

        Ok(match ResourceHelper::deserialize(deserializer)? {
            ResourceHelper::Simple(name) =>
            Resource {
                name: name.clone(),
                type_: ResourceType::from_str(&name).unwrap(), // auto-convert if needed
                types: None,
                id_prefixes: None,
            },
            ResourceHelper::Full(full) => Resource {
                name: full.name.clone(),
                type_: ResourceType::from_str(&full.name.clone()).unwrap(),
                types: None,
                id_prefixes: full.id_prefixes,
            },
        })
    }
}

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
    pub async fn new(url: String, client: &ClientWithMiddleware) -> Result<Self> {
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

   // #[instrument]
    pub async fn get_resources(
        &self,
        client: ClientWithMiddleware,
        //resource_type: ResourceType,
        imdb_id: &String,
        media_type: &MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Resources> {
        Ok(Resources {
            streams: {

                if self.manifest.resources.clone().into_iter().find(|x| x.type_ == ResourceType::Stream).is_some() {

                    Some(
                        self.get_streams(client.clone(), imdb_id, media_type, season, episode)
                            .await?
                            .streams
                            .unwrap()
                            .into_iter()
                            .filter(|x| x.is_valid())
                            .collect(),
                    )
                } else {
                    None
                }
            },
            // subtitles: if self.manifest.resources.contains(&ResourceType::Subtitles) {
            //     self.get_subtitles(client, imdb_id, media_type, season, episode)
            //         .await?
            //         .subtitles
            // } else {
            subtitles: None,
            metas: None,
        })
        // let streams = if ResourceType::Stream in self.resources
    }

    #[instrument]
    pub async fn get_streams(
        &self,
        client: ClientWithMiddleware,
        //resource_type: ResourceType,
        imdb_id: &String,
        media_type: &MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Resources> {
        //dbg!(format!("{}/stream/{}/{}.json", self.url, media_type, imdb_id).as_str());
        let url = match media_type {

            MediaType::Series => format!(
                "{}/stream/{}/{}:{}:{}.json",
                self.url,
                media_type,
                imdb_id,
                season.unwrap(),
                episode.unwrap()
            ),
                        _ => format!("{}/stream/{}/{}.json", self.url, media_type, imdb_id),
        };
        dbg!(&url);

        Ok(client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json::<Resources>()
            .await?)
    }

    pub async fn get_subtitles(
        &self,
        client: ClientWithMiddleware,
        //resource_type: ResourceType,
        imdb_id: &String,
        //filename: String,
        media_type: &MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Resources> {
        Ok(client
            .get(format!("{}/subtitles/{}/{}.json", self.url, media_type, imdb_id).as_str())
            //  .get(format!("{}/subtitles/{}/filename={}.json", self.url, imdb_id, filename).as_str())
            .send()
            .await?
            .error_for_status()?
            .json::<Resources>()
            .await?)
    }

    pub async fn get_catalogs(&self, client: ClientWithMiddleware) -> Result<Vec<Catalog>> {
        let catalogs = self
            .manifest
            .catalogs
            //  .as_ref()
            .clone()
            .unwrap_or_default();
        //.ok_or_else(|| eyre::eyre!("No catalogs found"))?;

        // Create a vector of futures
        let futures = catalogs.iter().map(|catalog| {
            let client = client.clone(); // Clone client for the async move block
            let catalog = catalog.clone(); // Clone if Catalog is Clone
            async move {
                let resources = self.get_catalog_items(client, catalog.clone()).await?;
                Ok(Catalog {
                    items: resources.metas,
                    addon_manifest: Some(self.manifest.clone()),
                    ..catalog
                })
            }
        });

        // Await all futures concurrently
        try_join_all(futures).await
    }

    pub async fn get_catalog_items(
        &self,
        client: ClientWithMiddleware,
        catalog: Catalog,
    ) -> Result<Resources> {
        Ok(client
            .get(
                format!(
                    "{}/catalog/{}/{}Catalog.json",
                    self.url, catalog.type_, catalog.id
                )
                .as_str(),
            )
            .send()
            .await?
            .error_for_status()?
            .json::<Resources>()
            .await?)
    }
}

#[derive(Debug, Clone)]
pub struct StremioClient {
    pub addons: Vec<Addon>,
    pub client: ClientWithMiddleware,
}

#[derive(Debug, Clone)]
pub struct ResourceOptions {
    pub addons: Vec<Addon>,
    pub client: ClientWithMiddleware,
}

impl StremioClient {
    pub async fn new(addon_urls: Vec<String>) -> Result<Self> {
        //let client = reqwest::Client::builder().build()?;
        let client = ClientBuilder::new(Client::new())
            .with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager: CACacheManager {
                    path: "/tmp/cacachee".into(),
                },
                options: HttpCacheOptions::default(),
            }))
            .build();
        let addons = join_all(addon_urls.into_iter().map(|x| {
            let c = client.clone();
            async move { Addon::new(x, &c).await.unwrap() }
        }))
        .await;

        Ok(Self { addons, client })
    }

    pub async fn get_catalogs(&self) -> Result<Vec<Catalog>> {
        Ok(join_all(
            self.addons
                .iter()
                .filter(|x| x.manifest.resources.clone().into_iter().find(|x| x.type_ == ResourceType::Catalog).is_some())
                .map(|addon| {
                    let c = self.client.clone();
                    async move {
                        // let catalogs: Vec<Catalog> = vec
                        //addon.get_catalogs(c).await.unwrap()
                        let catalogs = addon.get_catalogs(c).await.unwrap_or_else(|err| {
                            tracing::error!("Failed to get catalogs: {:?}", err);
                            Vec::new() // or any other default
                        });
                        catalogs
                        // }
                    }
                }),
        )
        .await
        .into_iter()
        .flatten()
        .collect())
    }

    pub async fn get_resources(
        &self,
        imdb_id: &String,
        media_type: &MediaType,
        //  resource_types: &[ResourceType],
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Vec<Resources>> {
        Ok(join_all(self.addons.iter().map(|addon| {
            let c = self.client.clone();
            async move {
                addon
                    .get_resources(c, imdb_id, media_type, season, episode)
                    .await
                    .unwrap()
            }
        }))
        .await)
        //.into_iter()
        //.flatten()
        //.collect()
    }

    pub async fn get_resources_flatten(
        &self,
        imdb_id: &String,
        media_type: &MediaType,
        //  resource_types: &[ResourceType],
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Resources> {
        let resources = self
            .get_resources(imdb_id, media_type, season, episode)
            .await?;
        Ok(resources
            .into_iter()
            .fold(Resources::default(), |mut acc, res| {
                if let Some(mut s) = res.streams {
                    acc.streams.get_or_insert_with(Vec::new).append(&mut s);
                }
                if let Some(mut sub) = res.subtitles {
                    acc.subtitles.get_or_insert_with(Vec::new).append(&mut sub);
                }
                acc
            }))
        //.into_iter()
        //.flatten()
        //.collect()
    }
}

use std::collections::HashMap;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Resources {
    pub streams: Option<Vec<Stream>>,
    pub subtitles: Option<Vec<Subtitle>>,
    // #[serde(rename = "metas")]
    pub metas: Option<Vec<CatalogItem>>,
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
    pub addon_manifest: Option<Manifest>,
}

impl Catalog {
    pub fn guid(&self) -> String {
        let addon_id = self
            .addon_manifest
            .as_ref()
            .expect("addon_manifest is None")
            .id
            .as_str();

        format!("catalog:{}:{}", addon_id, self.id)
    }
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

//impl Resource for Catalog {
//    fn path(&self) -> String {
//        format!("{}, by {} ({})", self.headline, self.author, self.location)
//    }
//}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Subtitle {
    pub id: String,
    pub url: String,
    pub sub_encoding: Option<String>,
    pub lang: Option<String>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub name: Option<String>,
    pub title: Option<String>,
    pub description: Option<String>,
    pub file_idx: Option<i64>,
    pub url: Option<String>,
    pub info_hash: Option<String>,
    pub behavior_hints: Option<behaviorHints>,
}

impl Stream {
    pub fn id(&self) -> String {
        let hints = self.behavior_hints.clone().unwrap();
        let s = format!(
            "{}{}{}",
            hints.video_size.unwrap_or(0),
            hints.binge_group.unwrap_or("".to_string()),
            hints.filename.unwrap_or("".to_string())
        );
        URL_SAFE.encode(s)
    }

    pub fn is_valid(&self) -> bool {
        self.url.is_some()
            && self
                .behavior_hints
                .as_ref()
                .map(|h| h.binge_group.is_some() && h.filename.is_some())
                .unwrap_or(false)
    }

    pub fn into_media_source(&self) -> jellyfin::MediaSourceInfo {
        let mut video = jellyfin::MediaStream {
            type_: Some(jellyfin::MediaStreamType::Video),
            ..Default::default()
        };

        let mut audio = jellyfin::MediaStream {
            type_: Some(jellyfin::MediaStreamType::Audio),
            ..Default::default()
        };

        let mut has_audio = false;

        if let Some(binge_group) = self
            .behavior_hints
            .as_ref()
            .and_then(|h| h.binge_group.as_ref())
        {
            let parts: Vec<&str> = binge_group.split('|').collect();
            let upper_parts: Vec<String> = parts.iter().map(|s| s.to_uppercase()).collect();
            //dbg!(&upper_parts);
            // Resolution → width/height
            if let Some(res) = upper_parts
                .iter()
                .find_map(|s| s.strip_suffix("P")?.parse::<i32>().ok())
            {
                video.height = Some(res);
                video.width = Some(((res as f32 * 16.0 / 9.0).round() as i32));
            }

            // HDR type
            // todo this could be essier with strum
            video.video_range_type = upper_parts.iter().find_map(|part| {
                match part.as_str() {
                    "HDR10+" => Some(jellyfin::VideoRangeType::Hdr10Plus),
                    "HDR10" => Some(jellyfin::VideoRangeType::Hdr10),
                    "HLG" => Some(jellyfin::VideoRangeType::Hlg),
                    "DOLBY VISION" | "DOLBYVISION" | "DV" => Some(jellyfin::VideoRangeType::Dovi),
                    "DOVIWITHHDR10" => Some(jellyfin::VideoRangeType::DoviWithHdr10),
                    "DOVIWITHHLG" => Some(jellyfin::VideoRangeType::DoviWithHlg),
                    "DOVIWITHSDR" => Some(jellyfin::VideoRangeType::DoviWithSdr),
                    "SDR" => Some(jellyfin::VideoRangeType::Sdr),
                    "HDR" => Some(jellyfin::VideoRangeType::Hdr10), // fallback if not more specific
                    _ => None,
                }
            });

            // Video codec
            let video_codecs = ["HEVC", "H265", "AVC", "H264", "AV1", "VP9"];
            if let Some(codec) = upper_parts
                .iter()
                .find(|s| video_codecs.iter().any(|c| s.contains(c)))
            {
                video.codec = Some(
                    match codec.as_str() {
                        "H265" => "HEVC",
                        "H264" => "AVC",
                        other => other,
                    }
                    .to_string(),
                );
            }

            // Audio codec
            let audio_formats = [
                ("ATMOS", "Atmos"),
                ("TRUEHD", "TrueHD"),
                ("EAC3", "EAC3"),
                ("AC3", "AC3"),
                ("DD", "AC3"),
                ("DTS", "DTS"),
                ("AAC", "AAC"),
                ("MP3", "MP3"),
            ];
            if let Some((_, codec)) = upper_parts.iter().find_map(|s| {
                audio_formats
                    .iter()
                    .find(|(key, _)| s.contains(*key))
                    .map(|x| (s, x.1))
            }) {
                audio.codec = Some(codec.to_string());
                has_audio = true;
            }

            // Channels (e.g. "7.1")
            if let Some(channel_str) = upper_parts.iter().find(|s| s.contains('.')) {
                if let Some((main, sub)) = channel_str.split_once('.') {
                    if let (Ok(main), Ok(sub)) = (main.parse::<i32>(), sub.parse::<i32>()) {
                        audio.channels = Some(main + sub);
                        has_audio = true;
                    }
                }
            }
        }

        // Determine file extension
        let container = self
            .behavior_hints
            .as_ref()
            .and_then(|h| h.filename.as_ref())
            .and_then(|f| f.split('.').last())
            .map(|s| s.to_lowercase());

        jellyfin::MediaSourceInfo {
            id: Some(self.id()),
            e_tag: Some(self.id()),
            //path: self.url.clone(),
            container,
            // protocol: Some("http".to_string()),
            supports_transcoding: Some(false),
            supports_direct_stream: Some(true),
            supports_direct_play: Some(true),
            //is_remote: Some(true),
            name: self.name.clone(),
            media_streams: Some({
                let mut streams = vec![video];
                if has_audio {
                    streams.push(audio);
                }
                streams
            }),
            ..Default::default()
        }
    }

    pub fn probe(&self) -> Result<jellyfin::MediaSourceInfo> {
        let id = self.id();

        debug!("Probing: {}", self.url.clone().unwrap());
        let info = ffprobe::ffprobe(self.url.clone().unwrap())?;

        //dbg!(&info);
        let mut source: jellyfin::MediaSourceInfo = info.into();
        source.id = Some(id.clone());
        source.e_tag = Some(id.clone());
        Ok(source)
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct behaviorHints {
    pub video_size: Option<i64>,
    pub binge_group: Option<String>,
    pub filename: Option<String>,
}
