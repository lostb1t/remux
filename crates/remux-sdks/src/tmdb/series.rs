use serde::{Deserialize, Serialize};

use super::{Status, default_append_to_response};
use crate::Endpoint;

use chrono::NaiveDate;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct Series {
    pub adult: bool,
    pub backdrop_path: Option<String>,
    pub created_by: Option<Vec<super::Creator>>,
    // pub episode_run_time: Vec<u32>,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub first_air_date: Option<NaiveDate>,
    //pub genres: Vec<Genre>,
    pub homepage: Option<String>,
    pub id: i64,
    //pub in_production: bool,
    // pub languages: Vec<String>,
    pub last_air_date: Option<String>,
    // pub last_episode_to_air: Option<Episode>,
    pub name: String,
    //pub next_episode_to_air: Option<Episode>,
    //pub networks: Option<Vec<Network>>,
    //  pub number_of_episodes: u32,
    // pub number_of_seasons: u32,
    pub origin_country: Vec<String>,
    pub original_language: String,
    pub original_name: String,
    pub overview: Option<String>,
    pub popularity: f64,
    pub poster_path: Option<String>,
    pub genres: Option<Vec<super::Genre>>,
    // pub production_companies: Vec<ProductionCompany>,
    // pub production_countries: Vec<ProductionCountry>,
    pub seasons: Vec<Season>,
    // pub spoken_languages: Vec<SpokenLanguage>,
    pub status: Option<Status>,
    pub tagline: Option<String>,
    pub r#type: String,
    pub vote_average: Option<f64>,
    pub vote_count: u32,
    pub external_ids: Option<super::ExternalIds>,
    pub credits: Option<super::Credits>,
    pub images: Option<super::Images>,
    pub content_ratings: Option<SeriesContentRatings>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesContentRatings {
    pub results: Vec<SeriesContentRating>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesContentRating {
    pub iso_3166_1: String,
    pub rating: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesEndpoint {
    pub id: i64,

    // #[builder(default = "en")]
    pub language: Option<String>,

    // #[builder(default = default_append_to_response())]
    pub append_to_response: Vec<String>,
}

impl SeriesEndpoint {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            language: Some("en".to_string()),
            append_to_response: default_append_to_response(),
        }
    }
}

impl Endpoint for SeriesEndpoint {
    type Output = Series;

    fn path(&self) -> String {
        format!("tv/{}", self.id)
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut params = vec![];
        if let Some(lang) = &self.language {
            params.push(("language".to_string(), lang.clone()));
        }
        if !self.append_to_response.is_empty() {
            params.push((
                "append_to_response".to_string(),
                self.append_to_response.join(","),
            ));
        }
        params
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Season {
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub air_date: Option<NaiveDate>,
    pub episode_count: Option<u32>,
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub poster_path: Option<String>,
    pub season_number: i64,
    pub vote_average: Option<f64>,
    pub episodes: Option<Vec<Episode>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeasonEndpoint {
    pub series_id: i64,
    pub season_number: i64,

    // #[builder(default = "en")]
    pub language: Option<String>,

    //#[builder(default = "Some(vec![\"images\".to_string(), \"external_ids\".to_string()])")]
    pub append_to_response: Option<Vec<String>>,
}

impl SeasonEndpoint {}

impl Endpoint for SeasonEndpoint {
    type Output = Season;

    fn path(&self) -> String {
        format!("tv/{}/season/{}", self.series_id, self.season_number)
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct Episode {
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub air_date: Option<NaiveDate>,
    pub episode_number: i64,
    pub episode_type: Option<String>,
    pub production_code: Option<String>,
    pub runtime: Option<i64>,
    pub season_number: i64,
    pub show_id: Option<i64>,
    pub still_path: Option<String>,
    pub credits: Option<super::Credits>,
    pub external_ids: Option<super::ExternalIds>,
    pub guest_stars: Option<Vec<super::CastMember>>,
    /// Populated when `append_to_response=images` is requested.
    /// Episodes return `stills` (high-res frames).
    pub images: Option<super::Images>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeEndpoint {
    pub series_id: i64,
    pub season_number: i64,
    pub episode_number: i64,

    // #[builder(default = "en")]
    pub language: Option<String>,

    //#[builder(default = "Some(vec![\"images\".to_string(), \"external_ids\".to_string()])")]
    pub append_to_response: Option<Vec<String>>,
}

impl EpisodeEndpoint {
    pub fn new(series_id: i64, season_number: i64, episode_number: i64) -> Self {
        Self {
            series_id,
            season_number,
            episode_number,
            language: None,
            append_to_response: Some(super::default_append_to_response()),
        }
    }
}

impl Endpoint for EpisodeEndpoint {
    type Output = Episode;

    fn path(&self) -> String {
        format!(
            "tv/{}/season/{}/episode/{}",
            self.series_id, self.season_number, self.episode_number
        )
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut params = vec![];
        if let Some(lang) = &self.language {
            params.push(("language".to_string(), lang.clone()));
        }
        if let Some(append) = &self.append_to_response {
            if !append.is_empty() {
                params.push(("append_to_response".to_string(), append.join(",")));
            }
        }
        params
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesSearchResult {
    pub id: i64,
    pub name: String,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub first_air_date: Option<NaiveDate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesSearchResponse {
    pub results: Vec<SeriesSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchTvEndpoint {
    pub query: String,
}

impl Endpoint for SearchTvEndpoint {
    type Output = SeriesSearchResponse;

    fn path(&self) -> String {
        "search/tv".to_string()
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![("query".to_string(), self.query.clone())]
    }
}
