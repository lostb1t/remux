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
    #[serde(skip)]
    pub external_id: String,
    pub external_source: String,
}

impl Endpoint for FindByIdEndpoint {
    type Output = FindByIdResponse;

    fn path(&self) -> String {
        format!("find/{}", self.external_id)
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
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
        "credits".to_string(),
        "release_dates".to_string(),
        "content_ratings".to_string(),
    ]
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ImageEntry {
    pub file_path: String,
    pub iso_639_1: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Images {
    #[serde(default)]
    pub logos: Vec<ImageEntry>,
    /// Posters (movies/series — vertical artwork).
    #[serde(default)]
    pub posters: Vec<ImageEntry>,
    /// Backdrops (movies/series — wide hero artwork).
    #[serde(default)]
    pub backdrops: Vec<ImageEntry>,
    /// Stills (episode-level horizontal frames). TMDB returns these when
    /// asked under `tv/{id}/season/{s}/episode/{e}/images`.
    #[serde(default)]
    pub stills: Vec<ImageEntry>,
}

impl Images {
    /// Best English logo, falling back to any language.
    pub fn best_logo(&self) -> Option<&str> {
        self.logos
            .iter()
            .filter(|e| {
                e.iso_639_1
                    .as_deref()
                    == Some("en")
            })
            .max_by(|a, b| {
                a.vote_average
                    .partial_cmp(&b.vote_average)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .or_else(|| {
                self.logos
                    .iter()
                    .next()
            })
            .map(|e| {
                e.file_path
                    .as_str()
            })
    }

    /// Best English-language backdrop (title card with text overlay).
    /// Returns None if no English-tagged backdrop exists.
    pub fn best_thumb(&self) -> Option<&str> {
        self.backdrops
            .iter()
            .filter(|e| {
                e.iso_639_1
                    .as_deref()
                    == Some("en")
            })
            .max_by(|a, b| {
                a.vote_average
                    .partial_cmp(&b.vote_average)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|e| {
                e.file_path
                    .as_str()
            })
    }
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Creator {
    pub id: u64,
    pub credit_id: String,
    pub name: String,
    pub original_name: String,
    pub gender: Option<u8>,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CastMember {
    pub id: i64,
    pub name: String,
    pub character: Option<String>,
    pub profile_path: Option<String>,
    pub order: i32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct CrewMember {
    pub id: i64,
    pub name: String,
    pub job: String,
    pub department: String,
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Credits {
    pub cast: Vec<CastMember>,
    pub crew: Vec<CrewMember>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
    pub tvdb_id: Option<i64>,
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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ProductionCompany {
    pub id: u64,
    pub logo_path: Option<String>,
    pub name: String,
    pub origin_country: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct PaginatedResponse<T> {
    pub page: u32,
    pub results: Vec<T>,
    pub total_pages: u32,
    pub total_results: u32,
}

#[derive(Debug, Clone, Default, Serialize)]
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
    #[serde(rename = "vote_count.gte")]
    pub vote_count_gte: Option<u32>,
    #[serde(rename = "vote_average.gte")]
    pub vote_average_gte: Option<f32>,

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

/// TMDB person search result entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PersonResult {
    pub id: i64,
    pub name: String,
    pub profile_path: Option<String>,
    pub known_for_department: Option<String>,
    pub popularity: Option<f64>,
}

/// `GET /search/person?query=…`
#[derive(Debug, Clone, Serialize)]
pub struct PersonSearchEndpoint {
    pub query: String,
}

impl Endpoint for PersonSearchEndpoint {
    type Output = PaginatedResponse<PersonResult>;

    fn path(&self) -> String {
        "search/person".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

/// TMDB person details response.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct PersonDetails {
    pub id: i64,
    pub name: String,
    pub biography: Option<String>,
    pub birthday: Option<String>,
    pub place_of_birth: Option<String>,
    pub imdb_id: Option<String>,
    pub profile_path: Option<String>,
}

/// `GET /person/{person_id}`
#[derive(Debug, Clone, Serialize)]
pub struct PersonDetailsEndpoint {
    pub person_id: i64,
}

impl Endpoint for PersonDetailsEndpoint {
    type Output = PersonDetails;

    fn path(&self) -> String {
        format!("person/{}", self.person_id)
    }
}

/// `GET /discover/movie`
#[derive(Debug, Clone)]
pub struct DiscoverMovieEndpoint {
    pub query: DiscoverQuery,
}

impl Endpoint for DiscoverMovieEndpoint {
    type Output = PaginatedResponse<movie::MovieSearchResult>;

    fn path(&self) -> String {
        "discover/movie".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        &self.query
    }
}

/// `GET /discover/tv`
#[derive(Debug, Clone)]
pub struct DiscoverTvEndpoint {
    pub query: DiscoverQuery,
}

impl Endpoint for DiscoverTvEndpoint {
    type Output = PaginatedResponse<series::SeriesSearchResult>;

    fn path(&self) -> String {
        "discover/tv".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        &self.query
    }
}

/// Time window for TMDB trending endpoints.
#[derive(Debug, Clone, Copy)]
pub enum TrendingWindow {
    Day,
    Week,
}

impl TrendingWindow {
    fn as_str(self) -> &'static str {
        match self {
            Self::Day => "day",
            Self::Week => "week",
        }
    }
}

/// `GET /trending/movie/{window}`
#[derive(Debug, Clone)]
pub struct TrendingMovieEndpoint {
    pub window: TrendingWindow,
    pub page: Option<u32>,
}

impl Endpoint for TrendingMovieEndpoint {
    type Output = PaginatedResponse<movie::MovieSearchResult>;

    fn path(&self) -> String {
        format!(
            "trending/movie/{}",
            self.window
                .as_str()
        )
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        [(
            "page",
            self.page
                .map(|p| p.to_string()),
        )]
    }
}

/// `GET /trending/tv/{window}`
#[derive(Debug, Clone)]
pub struct TrendingTvEndpoint {
    pub window: TrendingWindow,
    pub page: Option<u32>,
}

impl Endpoint for TrendingTvEndpoint {
    type Output = PaginatedResponse<series::SeriesSearchResult>;

    fn path(&self) -> String {
        format!(
            "trending/tv/{}",
            self.window
                .as_str()
        )
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        [(
            "page",
            self.page
                .map(|p| p.to_string()),
        )]
    }
}

/// A single streaming/rental/purchase provider entry.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WatchProvider {
    pub provider_id: i64,
    pub provider_name: String,
}

/// Per-country availability from `/watch/providers`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WatchProviderCountry {
    #[serde(default)]
    pub flatrate: Vec<WatchProvider>,
    #[serde(default)]
    pub rent: Vec<WatchProvider>,
    #[serde(default)]
    pub buy: Vec<WatchProvider>,
}

/// Response for `/movie/{id}/watch/providers` and `/tv/{id}/watch/providers`.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct WatchProvidersResponse {
    pub id: i64,
    /// Keys are ISO 3166-1 alpha-2 country codes.
    #[serde(default)]
    pub results: std::collections::HashMap<String, WatchProviderCountry>,
}

/// `GET /movie/{movie_id}/watch/providers`
#[derive(Debug, Clone, Serialize)]
pub struct MovieWatchProvidersEndpoint {
    #[serde(skip)]
    pub movie_id: i64,
}

impl Endpoint for MovieWatchProvidersEndpoint {
    type Output = WatchProvidersResponse;

    fn path(&self) -> String {
        format!("movie/{}/watch/providers", self.movie_id)
    }
}

/// `GET /tv/{series_id}/watch/providers`
#[derive(Debug, Clone, Serialize)]
pub struct TvWatchProvidersEndpoint {
    #[serde(skip)]
    pub series_id: i64,
}

impl Endpoint for TvWatchProvidersEndpoint {
    type Output = WatchProvidersResponse;

    fn path(&self) -> String {
        format!("tv/{}/watch/providers", self.series_id)
    }
}

//https://files.tmdb.org/p/exports/movie_ids_05_15_2024.json.gz
