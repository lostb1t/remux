use crate::{CachedEndpoint, ClientError, Endpoint, RestClient};
use http::Method;

use anyhow::Result;
//use chrono::{DateTime, Utc};
use chrono::{DateTime, Duration, Utc};
use remux_utils as utils;
use serde::Deserializer;
use serde::de::Error as _;
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use std::collections::HashMap;
use std::str::FromStr;
use url::Url;
use uuid::Uuid;

#[derive(
    //  Default,
    //   strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum MediaType {
    Movie,
    Series,
    Tv,
    Events,

    // custom
    Album,
    Artist,
    Track,
    #[serde(untagged)]
    Unknown(String),
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
#[strum(serialize_all = "lowercase")]
pub enum ResourceType {
    #[serde(alias = "streams")]
    Stream,
    Subtitles,
    Catalog,
    Meta,
    #[strum(to_string = "addon_catalog")]
    AddonCatalog,

    // custom
    Search,
    Lyrics,
    Segment,

    #[serde(untagged)]
    Unknown(String),
}

#[derive(Debug, Clone)]
pub struct ManifestEndpoint;

impl Endpoint for ManifestEndpoint {
    type Output = Manifest;

    fn path(&self) -> String {
        "/manifest.json".into()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: Option<String>,
    pub resources: Vec<Resource>,
    pub types: Vec<String>,
    #[serde(default)]
    pub catalogs: Vec<Catalog>,
    pub id_prefixes: Option<Vec<String>>,
    pub logo: Option<String>,
}

impl Manifest {
    pub fn get_catalog(&self, id: &str, kind: &String) -> Option<Catalog> {
        self.catalogs
            .iter()
            .find(|c| &c.kind == kind && c.id == id)
            .cloned()
    }

    pub fn get_search_catalog(&self, kind: &String) -> Option<Catalog> {
        self.catalogs
            .iter()
            .find(|c| &c.kind == kind && c.extra.iter().any(|e| e.name == "search"))
            .cloned()
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResourceRef {
    pub name: ResourceType,
    pub types: Vec<String>,
    pub id_prefixes: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub id: String,
    // this is a string because there isnt a fixed definition. Could be anythinf
    #[serde(rename = "type")]
    pub kind: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExtraProp {
    pub name: String,
    #[serde(default)]
    pub is_required: bool,
    #[serde(default, deserialize_with = "deserialize_options_skip_nulls")]
    pub options: Option<Vec<String>>,
}

fn deserialize_options_skip_nulls<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt: Option<Vec<Option<String>>> = Option::deserialize(deserializer)?;
    Ok(opt.map(|v| v.into_iter().flatten().collect()))
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize)]
pub struct CatalogEndpoint {
    #[serde(skip)]
    pub kind: String,
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogResponse {
    pub metas: Vec<Meta>,
}

// #[skip_serializing_none]
#[derive(Debug, Clone)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetaResponse {
    pub meta: Meta,
}

/// TODO: Add filename for better matching
#[derive(Debug, Clone)]
pub struct SubtitlesEndpoint {
    pub media_type: MediaType,
    pub imdb_id: String,
    pub season: Option<i64>,
    pub episode: Option<i64>,
}

impl Endpoint for SubtitlesEndpoint {
    type Output = SubtitlesResponse;

    fn path(&self) -> String {
        let id = match (self.season, self.episode) {
            (Some(s), Some(e)) => format!("{}:{}:{}", self.imdb_id, s, e),
            _ => self.imdb_id.clone(),
        };
        format!("/subtitles/{}/{}.json", self.media_type, id)
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Trailer {
    pub source: String,
    //pub r#type: String,
    //pub name: String,
    //pub lang: String,
}

#[derive(
    //   strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Default,
    PartialEq,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "lowercase")]
pub enum Status {
    Upcoming,
    Planned,
    Continuing,
    Ended,
    Canceled,
    #[serde(rename = "Returning Series")]
    ReturningSeries,
    #[serde(rename = "In Production")]
    InProduction,
    Running,
    #[default]
    #[serde(other)]
    Unknown,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Meta {
    // #[serde(alias = "imdb_id", alias = "imdbId")]
    #[serde(rename = "imdb_id")]
    pub imdb_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_option_string_or_array")]
    pub country: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_option_string_or_array")]
    pub director: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "deserialize_option_string_or_array",
        alias = "actors"
    )]
    pub cast: Option<Vec<String>>,
    #[serde(
        default,
        rename = "writer",
        alias = "writers",
        deserialize_with = "deserialize_option_string_or_array"
    )]
    pub writer: Option<Vec<String>>,
    pub description: Option<String>,
    pub genre: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_option_number_from_string"
    )]
    pub imdb_rating: Option<f64>,
    pub name: Option<String>,
    pub title: Option<String>,
    pub status: Option<Status>,
    pub released: Option<DateTime<Utc>>,
    pub slug: Option<String>,
    #[serde(rename = "type")]
    pub media_type: MediaType,
    pub certification: Option<String>,
    //#[serde(deserialize_with = "deserialize_string_from_number")]
    //pub year: String,
    pub moviedb_id: Option<u64>,

    pub trailers: Option<Vec<Trailer>>,

    pub background: Option<String>,
    pub logo: Option<String>,
    pub poster: Option<String>,
    pub thumbnail: Option<String>,

    pub awards: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_option_number_from_string"
    )]
    pub popularity: Option<f64>,
    pub id: String,
    pub genres: Option<Vec<String>>,
    // pub season_posters: Option<Vec<String>>,
    // this can be a range 2012-2015
    // #[serde(deserialize_with = "deserialize_string_from_number")]
    //pub release_info: String,
    #[serde(default, deserialize_with = "deserialize_opt_duration_empty_ok")]
    pub runtime: Option<Duration>,

    // #[serde(rename = "videos")]
    pub videos: Option<Vec<Episode>>,
    // pub trailer_streams: Option<Vec<String>>,
    // pub links: Option<Vec<Link>>,
    #[serde(
        default,
        rename = "app_extras",
        deserialize_with = "deserialize_app_extras"
    )]
    pub app_extras: Option<AppExtras>,
}

impl Meta {
    /// Fetch the full meta from AIO and replace `self` with it.
    /// Catalog responses are often partial (missing `imdb_id` etc.); calling
    /// this upgrades the item to complete metadata before DB conversion.
    pub async fn resolve(&mut self, client: &RestClient) -> Result<()> {
        *self = client
            .execute(
                MetaEndpoint {
                    media_type: self.media_type.clone(),
                    id: self.id.clone(),
                    season: None,
                    episode: None,
                }
                .with_cache(std::time::Duration::from_secs(3600)),
            )
            .await?
            .meta;
        Ok(())
    }

    pub fn get_name(&self) -> Option<String> {
        self.name.clone().or_else(|| self.title.clone())
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppExtras {
    #[serde(default, deserialize_with = "deserialize_option_cast_members")]
    pub cast: Option<Vec<CastMember>>,
    #[serde(default, deserialize_with = "deserialize_option_cast_members")]
    pub directors: Option<Vec<CastMember>>,
    #[serde(default, deserialize_with = "deserialize_option_cast_members")]
    pub writers: Option<Vec<CastMember>>,
    pub season_posters: Option<Vec<Option<String>>>,
    pub certification: Option<String>,
    pub release_dates: Option<ReleaseDates>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseDates {
    pub results: Vec<ReleaseDateCountry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseDateCountry {
    pub iso_3166_1: String,
    pub release_dates: Vec<ReleaseDateEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReleaseDateEntry {
    pub release_date: DateTime<Utc>,
    #[serde(rename = "type")]
    pub release_type: u8,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastMember {
    pub name: Option<String>,
    pub character: Option<String>,
    pub photo: Option<String>,
}

//use std::time::Duration;

fn deserialize_app_extras<'de, D>(de: D) -> Result<Option<AppExtras>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = Option::<serde_json::Value>::deserialize(de)?;
    match raw {
        None => Ok(None),
        Some(v) => Ok(serde_json::from_value(v).ok()),
    }
}

/// Accepts either a JSON string or an array of strings.
/// A bare string becomes a single-element Vec; null or missing becomes None.
fn deserialize_option_string_or_array<'de, D>(
    de: D,
) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        S(String),
        V(Vec<String>),
    }
    Ok(Option::<Repr>::deserialize(de)?.map(|r| match r {
        Repr::S(s) => vec![s],
        Repr::V(v) => v,
    }))
}

pub fn deserialize_option_cast_members<'de, D>(
    de: D,
) -> Result<Option<Vec<CastMember>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Item {
        S(String),
        O(CastMember),
    }

    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Repr {
        Single(Item),
        Array(Vec<Item>),
    }

    Ok(Option::<Repr>::deserialize(de)?.map(|r| match r {
        Repr::Single(Item::S(s)) => vec![CastMember {
            name: Some(s),
            character: None,
            photo: None,
        }],
        Repr::Single(Item::O(o)) => vec![o],
        Repr::Array(arr) => arr
            .into_iter()
            .map(|item| match item {
                Item::S(s) => CastMember {
                    name: Some(s),
                    character: None,
                    photo: None,
                },
                Item::O(o) => o,
            })
            .collect(),
    }))
}

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
                let std_duration = parse_duration_lossy(t).map_err(D::Error::custom)?;

                Ok(Some(
                    Duration::from_std(std_duration).map_err(D::Error::custom)?,
                ))
            }
        }
    }
}

fn parse_duration_lossy(input: &str) -> Result<std::time::Duration, String> {
    if let Ok(duration) = duration_str::parse(input) {
        return Ok(duration);
    }

    // Some AIO/Stremio catalogs emit malformed values like "31S min".
    // Normalize the known bad form and retry so one bad runtime does not
    // fail the entire page fetch.
    let normalized = input
        .replace("S min", " min")
        .replace("s min", " min")
        .replace("S mins", " mins")
        .replace("s mins", " mins");

    duration_str::parse(&normalized).map_err(|e| e.to_string())
}

impl Meta {
    pub fn is_series(&self) -> bool {
        self.media_type == MediaType::Series
    }

    pub fn get_season_numbers(&self) -> Vec<i64> {
        // dbg!(&self);
        if let Some(episodes) = self.videos.as_ref() {
            let mut seasons: Vec<i64> =
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

    pub fn get_episodes(&self, season_idx: i64) -> Vec<Episode> {
        self.videos
            .clone()
            .unwrap_or_default()
            .into_iter()
            .filter(|e| e.season.map_or(false, |s| s == season_idx))
            .collect()
    }

    pub fn get_season_poster(&self, idx: i64) -> Option<String> {
        self.app_extras
            .as_ref()
            .and_then(|extras| extras.season_posters.as_ref())
            .and_then(|posters| posters.get(idx as usize).cloned())
            .flatten()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Episode {
    pub id: String,
    pub title: Option<String>,
    pub name: Option<String>,
    pub released: Option<DateTime<Utc>>,
    pub thumbnail: Option<String>,
    pub episode: Option<i64>,
    pub season: Option<i64>,
    pub overview: Option<String>,
    pub number: Option<i64>,
    pub description: Option<String>,
    #[serde(
        default,
        deserialize_with = "crate::deserialize_option_number_from_string"
    )]
    pub rating: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_opt_duration_empty_ok")]
    pub runtime: Option<Duration>,
    pub directors: Option<Vec<String>>,
    pub writers: Option<Vec<String>>,
    pub cast: Option<Vec<CastMember>>,
}
impl Episode {
    pub fn get_name(&self) -> Option<String> {
        self.name.clone().or_else(|| self.title.clone())
    }
}

/// Standard Stremio streams endpoint: `GET /stream/{type}/{id}.json`
#[derive(Debug, Clone)]
pub struct StreamEndpoint {
    pub kind: MediaType,
    pub id: String,
}

impl Endpoint for StreamEndpoint {
    type Output = StreamsResponse;

    fn path(&self) -> String {
        format!("/stream/{}/{}.json", self.kind, self.id)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamsResponse {
    #[serde(default)]
    pub streams: Vec<Stream>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Stream {
    pub info_hash: Option<String>,
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
    #[serde(default)]
    pub proxied: bool,
    pub filename: Option<String>,
    pub folder_name: Option<String>,
    // pub size: i64,
    //pub folder_size: Option<i64>,
    pub message: Option<String>,
    #[serde(default)]
    pub library: bool,
    pub addon: Option<String>,
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    pub indexer: Option<String>,
    pub duration: Option<i64>,
    pub size: Option<i64>,
    pub video_hash: Option<String>,
    #[serde(default)]
    pub subtitles: Vec<Subtitle>,
    #[serde(default)]
    pub country_whitelist: Vec<String>,
    #[serde(default)]
    pub request_headers: HashMap<String, String>,
    #[serde(default)]
    pub response_headers: HashMap<String, String>,
    pub parsed_file: Option<ParsedFile>,
    pub name: Option<String>,
    pub description: Option<String>,
}

impl Stream {
    pub fn is_torrent(&self) -> bool {
        self.info_hash.is_some()
    }

    pub fn is_valid(&self) -> bool {
        if self.info_hash.is_some() {
            return true;
        }

        let url = match &self.url {
            Some(u) => u,
            None => return false,
        };

        if url.trim().is_empty() {
            return false;
        }

        let parsed = match Url::parse(url) {
            Ok(u) => u,
            Err(_) => return false,
        };

        let path = parsed.path();

        !(path == "/" || path.is_empty())
    }

    pub fn id(&self) -> String {
        self.info_hash.clone().unwrap()
    }

    pub fn get_guid(&self) -> Uuid {
        let key = if let Some(hash) = &self.info_hash {
            hash.to_string()
        } else if let Some(filename) = &self.filename {
            format!("{}{}", filename, self.size.unwrap_or_default())
        } else {
            self.url.clone().unwrap()
        };

        utils::get_stable_uuid(key)
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

    #[serde(default)]
    pub season_pack: bool,
}

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?)
}

#[cfg(test)]
mod tests {
    use super::parse_duration_lossy;
    use std::time::Duration;

    #[test]
    fn parses_standard_duration_strings() {
        assert_eq!(
            parse_duration_lossy("31 min").unwrap(),
            Duration::from_secs(31 * 60)
        );
    }

    #[test]
    fn tolerates_stremio_runtime_typo() {
        assert_eq!(
            parse_duration_lossy("31S min").unwrap(),
            Duration::from_secs(31 * 60)
        );
    }
}
