use serde::{Deserialize, Serialize, Serializer};
use serde_with::skip_serializing_none;

use super::{Status, default_append_to_response};
use crate::Endpoint;

use chrono::NaiveDate;

fn serialize_comma<S>(v: &[String], s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&v.join(","))
}

fn serialize_comma_opt<S>(v: &Option<Vec<String>>, s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match v {
        Some(list) => s.serialize_str(&list.join(",")),
        None => s.serialize_none(),
    }
}

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
    pub production_companies: Option<Vec<super::ProductionCompany>>,
    pub production_countries: Option<Vec<super::ProductionCountry>>,
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

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeriesEndpoint {
    #[serde(skip)]
    pub id: i64,
    pub language: Option<String>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_comma"
    )]
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

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
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

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodeEndpoint {
    #[serde(skip)]
    pub series_id: i64,
    #[serde(skip)]
    pub season_number: i64,
    #[serde(skip)]
    pub episode_number: i64,

    // #[builder(default = "en")]
    pub language: Option<String>,

    //#[builder(default = "Some(vec![\"images\".to_string(), \"external_ids\".to_string()])")]
    #[serde(serialize_with = "serialize_comma_opt")]
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

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SeriesSearchResult {
    pub id: i64,
    pub name: String,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub first_air_date: Option<NaiveDate>,
    pub poster_path: Option<String>,
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

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}
