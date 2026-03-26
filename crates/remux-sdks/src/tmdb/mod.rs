//use progenitor::generate_api;
//generate_api!("src/sdks/tmdb/openapi.yml");
//generate_api!(
//    spec = "src/sdks/tmdb/tmdb_openapi_3_0_validated.yaml",      // The OpenAPI document
//    interface = Builder
//);

use crate::Endpoint;
use serde::{Deserialize, Serialize};

pub mod movie;
pub use movie::*;
pub mod series;
pub use series::*;

pub trait IdSetter {
    fn id(self, id: i64) -> Self;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FindByIdEndpoint {
    pub external_id: String,
    pub external_source: String,
}

impl Endpoint for FindByIdEndpoint {
    type Output = FindByIdResponse;

    fn path(&self) -> String {
        format!("find/{}", self.external_id)
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![("external_source".to_string(), self.external_source.clone())]
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExternalIdType {
    ImdbId,
    FacebookId,
    InstagramId,
    TvdbId,
    TiktokId,
    TwitterId,
    WikidataId,
    YoutubeId,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct FindByIdResponse {
    pub movie_results: Vec<Movie>,
    pub tv_results: Vec<Series>,
}

// pub fn get_endpoint_for_media_type(t: media::MediaType) -> tmdb::MediaEndpoint {
//     match t {
//         MediaType::Movie => tmdb::MediaEndpoint::Movie,
//         MediaType::TVShow => tmdb::MediaEndpoint::TVShow,
//     }
// }

pub fn default_append_to_response() -> Vec<String> {
    vec![
        "genres".to_string(),
        "images".to_string(),
        "external_ids".to_string(),
    ]
}

#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Deserialize,
    serde::Serialize,
)]
pub enum Status {
    #[serde(rename = "Rumored")]
    Rumored,
    #[serde(rename = "Planned")]
    Planned,
    #[serde(rename = "In Production")]
    InProduction,
    #[serde(rename = "Post Production")]
    PostProduction,
    #[serde(rename = "Released")]
    Released,
    #[serde(rename = "Canceled")]
    Canceled,
    #[serde(rename = "Pilot")]
    Pilot,
    #[serde(rename = "Returning Series")]
    ReturningSeries,
    #[serde(rename = "Ended")]
    Ended,
}
//use serde::{Serialize, Deserialize};

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct Creator {
    pub id: u64,
    pub credit_id: String,
    pub name: String,
    pub original_name: String,
    pub gender: Option<u8>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Genre {
    pub id: u64,
    pub name: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Network {
    pub id: u64,
    pub logo_path: Option<String>,
    pub name: String,
    pub origin_country: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ProductionCompany {
    pub id: u64,
    pub logo_path: Option<String>,
    pub name: String,
    pub origin_country: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ProductionCountry {
    pub iso_3166_1: String,
    pub name: String,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct SpokenLanguage {
    pub english_name: String,
    pub iso_639_1: String,
    pub name: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PaginatedResponse<T> {
    pub page: u32,
    pub results: Vec<T>,
    pub total_pages: u32,
    pub total_results: u32,
}

#[derive(Debug, Default, Serialize)]
pub struct DiscoverQuery {
    // Common parameters
    pub language: Option<String>,
    pub sort_by: Option<String>,
    pub page: Option<u32>,
    pub timezone: Option<String>,
    pub include_null_first_air_dates: Option<bool>,
    pub with_watch_providers: Option<String>,
    pub watch_region: Option<String>,
    pub with_genres: Option<String>,
    pub with_keywords: Option<String>,
    pub with_runtime_gte: Option<u32>,
    pub with_runtime_lte: Option<u32>,
    pub with_original_language: Option<String>,
    pub without_genres: Option<String>,
    pub with_watch_monetization_types: Option<String>,
    pub without_keywords: Option<String>,
    pub with_status: Option<String>,
    pub with_type: Option<String>,
    pub with_networks: Option<String>,
    pub with_companies: Option<String>,
    pub with_origin_country: Option<String>,

    // Movie-specific parameters
    pub region: Option<String>,
    pub certification_country: Option<String>,
    pub certification: Option<String>,
    pub certification_lte: Option<String>,
    pub certification_gte: Option<String>,
    pub include_adult: Option<bool>,
    pub include_video: Option<bool>,
    pub primary_release_year: Option<u32>,
    pub primary_release_date_gte: Option<String>,
    pub primary_release_date_lte: Option<String>,
    pub release_date_gte: Option<String>,
    pub release_date_lte: Option<String>,
    pub with_release_type: Option<String>,
    pub year: Option<u32>,

    // TV-specific parameters
    pub first_air_date_year: Option<u32>,
    pub first_air_date_gte: Option<String>,
    pub first_air_date_lte: Option<String>,
    pub air_date_gte: Option<String>,
    pub air_date_lte: Option<String>,
    pub screened_theatrically: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SortBy {
    PopularityAsc,
    PopularityDesc,
    ReleaseDateAsc,
    ReleaseDateDesc,
    RevenueAsc,
    RevenueDesc,
    PrimaryReleaseDateAsc,
    PrimaryReleaseDateDesc,
    VoteAverageAsc,
    VoteAverageDesc,
    VoteCountAsc,
    VoteCountDesc,
    OriginalTitleAsc,
    OriginalTitleDesc,
    TitleAsc,
    TitleDesc,
}

//https://files.tmdb.org/p/exports/movie_ids_05_15_2024.json.gz
