use serde::{Deserialize, Serialize, Serializer};

use super::{Status, default_append_to_response};
use crate::Endpoint;

use chrono::NaiveDate;
use serde_with::{serde_as, skip_serializing_none};

fn serialize_comma<S>(v: &[String], s: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    s.serialize_str(&v.join(","))
}

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub overview: Option<String>,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub release_date: Option<NaiveDate>,
    pub runtime: Option<i64>,
    pub vote_average: Option<f64>,
    pub vote_count: Option<i64>,
    pub adult: bool,
    pub status: Option<Status>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub imdb_id: Option<String>,
    pub original_language: String,
    pub genres: Option<Vec<super::Genre>>,
    pub external_ids: Option<super::ExternalIds>,
    pub credits: Option<super::Credits>,
    pub images: Option<super::Images>,
    pub release_dates: Option<MovieReleaseDates>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieReleaseDates {
    pub results: Vec<MovieReleaseCountry>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieReleaseCountry {
    pub iso_3166_1: String,
    pub release_dates: Vec<MovieReleaseDate>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieReleaseDate {
    pub certification: Option<String>,
    pub release_date: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(rename = "type", default)]
    pub release_type: u8,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieEndpoint {
    #[serde(skip)]
    pub id: i64,
    pub language: Option<String>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_comma"
    )]
    pub append_to_response: Vec<String>,
}

impl MovieEndpoint {
    pub fn new(id: i64) -> Self {
        Self {
            id,
            language: Some("en".to_string()),
            append_to_response: default_append_to_response(),
        }
    }
}

impl Endpoint for MovieEndpoint {
    type Output = Movie;

    fn path(&self) -> String {
        format!("movie/{}", self.id)
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieSearchResult {
    pub id: i64,
    pub title: String,
    #[serde(default, deserialize_with = "crate::deserialize_option_naive_date")]
    pub release_date: Option<NaiveDate>,
    pub poster_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovieSearchResponse {
    pub results: Vec<MovieSearchResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchMovieEndpoint {
    pub query: String,
    pub year: Option<i64>,
}

impl Endpoint for SearchMovieEndpoint {
    type Output = MovieSearchResponse;

    fn path(&self) -> String {
        "search/movie".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        self
    }
}
