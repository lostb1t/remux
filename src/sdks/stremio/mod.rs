use crate::sdks::core::{CommaSeparatedList, Endpoint, QueryParams};
use crate::sdks::jellyfin;
use bon::Builder;
use eyre::Result;
use futures_util::future::join_all;
use futures_util::future::try_join_all;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::time::Duration;
use std::{collections::HashMap, path::Path};
//use crate::errors::Result;
use base64::{Engine as _, engine::general_purpose::URL_SAFE};
use eyre;
use std::str::FromStr;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct StremioService {
    pub addons: Vec<Addon>,
}

#[derive(Debug, Clone)]
pub struct Addon {
    pub url: String,
    pub manifest: Manifest,
    pub client: super::core::RestClient,
    //pub config: crate::AddonConfig
}

impl Addon {
    pub async fn new(u: String) -> Result<Self> {
        let url = u.replace("manifest.json", "");
        let client = super::core::RestClient::without_cache(&url)?;
        let mut manifest = ManifestEndpoint {}.query(&client).await?;
       //dbf
       //manifest.catalogs[0].uuid = "catalog:test".to_string();

        Ok(Self {
            url,
            manifest,
            client,
           // config
        })
    }

    pub fn catalog_guid(&self, c: &Catalog) -> String {
        format!("catalog:{}:{}", self.manifest.id, c.id)
    }

    pub async fn get_streams(
        &self,
        imdb_id: String,
        media_type: MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Vec<Stream>> {
        Ok(StreamEndpoint {
            imdb_id,
            media_type,
            season,
            episode,
        }
        .query(&self.client)
        .await?
        .streams
        .into_iter()
     //   .filter(|x| x.is_valid())
        .collect())
    }

    pub async fn get_meta(
        &self,
        imdb_id: String,
        media_type: MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Option<Meta>> {
        Ok(Some(
            MetaEndpoint {
                imdb_id,
                media_type,
                season,
                episode,
            }
            .query(&self.client)
            .await?
            .meta,
        ))
    }
}

impl StremioService {
    pub async fn new(config: Vec<String>) -> Result<Self> {
        let addons = join_all(
            config
                .into_iter()
                .map(|x| async move { Addon::new(x).await.unwrap() }),
        )
        .await;

        Ok(Self { addons })
    }

    pub fn get_search_catalog(&self, media_type: MediaType) -> Option<Catalog> {
        for addon in &self.addons {
            for catalog in &addon.manifest.catalogs {
                if catalog.kind == media_type && catalog.has_search() {
                    return Some(catalog.clone());
                }
            }
        }
        None
    }

    pub fn get_library_catalog(&self, media_type: MediaType) -> Option<Catalog> {
        for addon in &self.addons {
            for catalog in &addon.manifest.catalogs {
                if catalog.kind == media_type {
                    return Some(catalog.clone());
                }
            }
        }
        None
    }

    pub async fn get_streams(
        &self,
        imdb_id: String,
        media_type: MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Vec<Stream>> {
        Ok(join_all(self.addons.iter().map(|addon| {
            addon.get_streams(imdb_id.clone(), media_type.clone(), season, episode)
        }))
        .await
        .into_iter()
        .filter_map(Result::ok)
        .flatten()
        .collect())
    }

    pub async fn get_meta(
        &self,
        imdb_id: String,
        media_type: MediaType,
        season: Option<i64>,
        episode: Option<i64>,
    ) -> Result<Option<Meta>> {
        for addon in self.addons.iter() {
            if addon
                .manifest
                .resources
                .iter()
                .any(|r| r.resource_type() == ResourceType::Meta)
            {
                return addon.get_meta(imdb_id, media_type, season, episode).await;
            }
        }
        Ok(None)
    }

    pub fn get_catalogs(&self) -> Vec<Catalog> {
        self.addons
            .iter()
            .map(|addon| addon.manifest.catalogs.clone())
            .into_iter()
            .flatten()
            .collect()
    }

    pub fn get_catalog(&self, uuid: &str) -> Option<Catalog> {
        self.get_catalogs().into_iter().find(|c| c.uuid == uuid)
    }

    pub async fn get_catalog_items(
        &self,
        uuid: String,
        search: Option<String>,
        skip: Option<u32>,
    ) -> Result<Vec<Meta>> {
        // first find the addon + catalog that match this uuid
        let (addon, catalog) = self
            .addons
            .iter()
            .find_map(|addon| {
                addon
                    .manifest
                    .catalogs
                    .iter()
                    .find(|c| c.uuid == uuid)
                    .map(|cat| (addon, cat))
            })
            .ok_or_else(|| eyre::eyre!("catalog not found"))?;

        // now call get_items
        catalog.get_items(addon, search, skip).await
    }
}

/// A response is considered rate-limited if the addon returns a success status (200)
/// but the payload clearly indicates a rate-limit placeholder stream instead of real data.
pub trait RateLimited {
    fn is_rate_limited(&self) -> bool;
}

#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    pub max_retries: usize,
    pub backoff_first: Duration,
    /// Maximum delay cap for backoff (to avoid runaway sleeps)
    pub backoff_cap: Duration,
}

impl Default for RetryConfig {
    fn default() -> Self {
        Self {
            max_retries: 6,
            backoff_first: Duration::from_secs(2),
            backoff_cap: Duration::from_secs(60),
        }
    }
}

pub async fn query_with_ratelimit_retry<E>(
    client: &crate::sdks::core::RestClient,
    endpoint: &E,
    cfg: RetryConfig,
) -> Result<E::Output>
where
    E: crate::sdks::core::Endpoint + Sync,
    E::Output: RateLimited,
{
    let mut delay = cfg.backoff_first;
    for attempt in 0..=cfg.max_retries {
        let res = endpoint.query(client).await?;
        if !res.is_rate_limited() {
            return Ok(res);
        }
        if attempt == cfg.max_retries {
            break;
        }
        tracing::warn!(
            "Addon rate-limit body detected; retrying in {delay:?} (attempt {}/{})",
            attempt + 1,
            cfg.max_retries
        );
        tokio::time::sleep(delay).await;
        delay = std::cmp::min(delay * 2, cfg.backoff_cap);
    }
    Err(eyre::eyre!(
        "rate-limited after {} retries",
        cfg.max_retries
    ))
}

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

    fn endpoint(&self) -> String {
        "/manifest.json".into()
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

impl Manifest {}

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

    // we assign a uuid
    #[serde(default = "new_uuid")]
    pub uuid: String,
}

fn new_uuid() -> String {
    format!("catalog:{}", uuid::Uuid::new_v4().to_string())
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
  
    pub async fn get_items(
        &self,
        addon: &Addon,
        search: Option<String>,
        skip: Option<u32>,
    ) -> Result<Vec<Meta>> {
        let endpoint = CatalogEndpoint {
            kind: self.kind.clone(),
            id: self.id.clone(),
            search,
            genre: None,
            skip,
        };
        Ok(endpoint.query(&addon.client).await?.metas)
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

    fn endpoint(&self) -> String {
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

    //fn parameters(&self) -> QueryParams {
    //    HashMap::new()
    //}
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct CatalogResponse {
    pub metas: Vec<Meta>,
}

// #[skip_serializing_none]
#[derive(Debug, Clone, Builder)]
pub struct MetaEndpoint {
    pub media_type: MediaType,
    pub imdb_id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl Endpoint for MetaEndpoint {
    type Output = MetaResponse;

    fn endpoint(&self) -> String {
        let mut id = self.imdb_id.clone();
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

// #[skip_serializing_none]
#[derive(Debug, Clone, Builder)]
pub struct StreamEndpoint {
    pub media_type: MediaType,
    pub imdb_id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl Endpoint for StreamEndpoint {
    type Output = StreamResponse;

    fn endpoint(&self) -> String {
        format!("/stream/{}/{}.json", self.media_type, self.imdb_id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct StreamResponse {
    pub streams: Vec<Stream>,
}

impl RateLimited for StreamResponse {
    fn is_rate_limited(&self) -> bool {
        if self.streams.len() != 1 {
            return false;
        }
        let s = &self.streams[0];
        let title = s.title.as_deref().unwrap_or("").to_ascii_lowercase();
        let name = s.name.as_deref().unwrap_or("").to_ascii_lowercase();
        let url = s.url.as_deref().unwrap_or("");

        // Heuristics seen in AIOStreams rate-limit responses
        // 1) Title or name contains "rate-limit exceeded"
        if title.contains("rate-limit exceeded") || name.contains("rate-limit exceeded")
        {
            return true;
        }
        // 2) Known placeholder asset
        if url.contains("public-rate-limit-exceeded.mp4") {
            return true;
        }
        false
    }
}

/// aiostream only. So only use for testing
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
pub struct StreamData {
    pub id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
pub struct BehaviorHints {
    pub filename: Option<String>,
    pub binge_group: Option<String>,
    pub video_size: Option<u64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub title: Option<String>,
    pub name: Option<String>,
    pub description: Option<String>,
    pub url: Option<String>,
    pub external_url: Option<String>,
    pub behavior_hints: Option<BehaviorHints>,
    pub stream_data: Option<StreamData>,
}

impl Stream {
    // pub fn rate_limited(&self) -> bool {
    //     self.behavior_hints.is_none()
    // }

    /// could have used the url but these tend to be crazy long
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

    // pub fn is_valid(&self) -> bool {
    //     self.behavior_hints.is_some()
    // }

    pub fn filesize(&self) -> Option<u64> {
        self.behavior_hints.as_ref()?.video_size
    }

    // pub fn ext(&self) -> Option<String> {
    //     if let Some(filename) = self.filename() {
    //         return std::path::Path::new(&filename)
    //             .extension()
    //             .and_then(|e| e.to_str())
    //             .map(|s| s.to_string());
    //     }
    //     None
    // }

    // pub fn filename(&self) -> Option<String> {
    //     use std::path::Path;
    //     use url::Url;

    //     let decode_name =
    //         |name: &str| urlencoding::decode(name).unwrap_or_else(|_| name.to_string());

    //     let mut url_name: Option<String> = None;

    //     if let Some(url_str) = &self.url {
    //         if url_str.starts_with("magnet:?") {
    //             if let Ok(parsed) = Url::parse(url_str) {
    //                 if let Some(dn) = parsed.query_pairs().find(|(k, _)| k == "dn") {
    //                     url_name = Some(decode_name(&dn.1));
    //                 }
    //             }
    //         } else {
    //             let path = if let Some(path_start) = url_str.find("://") {
    //                 let path_part = &url_str[(path_start + 3)..];
    //                 path_part[path_part.find('/').unwrap_or(path_part.len())..].to_string()
    //             } else {
    //                 url_str.clone()
    //             };

    //             if let Some(name) = Path::new(&path).file_name().and_then(|n| n.to_str()) {
    //                 url_name = Some(decode_name(name));
    //             }
    //         }
    //     }

    //     // If URL filename has extension, return it
    //     if let Some(name) = &url_name {
    //         if Path::new(name).extension().is_some() {
    //             return Some(name.clone());
    //         }
    //     }

    //     // Otherwise, fall back to filename field on BehaviorHints struct
    //     if let Some(hints) = &self.behavior_hints {
    //         if let Some(filename_str) = hints.filename.as_deref() {
    //             return Some(decode_name(filename_str));
    //         }
    //     }

    //     url_name
    // }

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
            let upper_parts: Vec<String> =
                parts.iter().map(|s| s.to_uppercase()).collect();
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
                    "DOLBY VISION" | "DOLBYVISION" | "DV" => {
                        Some(jellyfin::VideoRangeType::Dovi)
                    }
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
                    if let (Ok(main), Ok(sub)) =
                        (main.parse::<i32>(), sub.parse::<i32>())
                    {
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

        super::jellyfin::MediaSourceInfo {
            id: Some(self.id()),
            e_tag: Some(self.id()),
            //path: self.url.clone(),
            container,
            // protocol: Some("http".to_string()),
            supports_transcoding: Some(false),
            supports_direct_stream: Some(true),
            supports_direct_play: Some(true),
            //is_remote: Some(true),
name: {
    match &self.description {
        Some(desc) => format!("{}\n{}", self.name.as_ref().unwrap(), desc),
        None => self.name.clone().unwrap(),
    }
}.into(),
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

    pub fn probe(&self) -> Result<super::jellyfin::MediaSourceInfo> {
        let id = self.id();

        // debug!("Probing: {}", self.url.clone().unwrap());
        let info = ffprobe::ffprobe(self.url.clone().unwrap())?;

        //dbg!(&info);
        let mut source: super::jellyfin::MediaSourceInfo = info.into();
        source.id = Some(id.clone());
        source.e_tag = Some(id.clone());
        Ok(source)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Builder)]
pub struct SubtitleResponse {
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
    pub imdb_id: String,
    pub country: Option<String>,
    pub description: Option<String>,
    pub genre: Option<Vec<String>>,
    pub imdb_rating: Option<String>,
    pub name: Option<String>,
    pub released: Option<String>,
    pub slug: Option<String>,
    #[serde(rename = "type")]
    pub media_type: MediaType,
    pub writer: Option<Vec<String>>,
    pub year: Option<String>,
    pub moviedb_id: Option<u64>,

    // pub popularities: Option<Popularities>,
    // pub trailers: Option<Vec<String>>,
    pub cast: Option<Vec<String>>,
    pub director: Option<Vec<String>>,
    pub background: Option<String>,
    pub logo: Option<String>,
    pub awards: Option<String>,
    pub popularity: Option<f64>,
    pub poster: Option<String>,
    pub id: Option<String>,
    pub genres: Option<Vec<String>>,
    pub release_info: Option<String>,
    // pub trailer_streams: Option<Vec<String>>,
    // pub links: Option<Vec<Link>>,
    // pub behavior_hints: Option<BehaviorHints>,
}
