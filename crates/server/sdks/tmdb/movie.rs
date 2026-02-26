use serde::{Deserialize, Serialize};

use super::{Status, default_append_to_response};
use crate::sdks::{CommaSeparatedList, Endpoint};

use chrono::NaiveDate;
use serde_with::{DisplayFromStr, serde_as};

#[serde_as]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub overview: Option<String>,
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MovieEndpoint {
    pub id: i64,

    // #[builder(default = "en")]
    pub language: Option<String>,

    // #[builder(default = default_append_to_response())]
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

    fn query(&self) -> Vec<(String, String)> {
        let mut params = vec![];
        if let Some(lang) = &self.language {
            params.push(("language".to_string(), lang.clone()));
        }
        // if let Some(appends) = &self.append_to_response {
        //params.push(("append_to_response".to_string(), self.append_to_response.join(",")));
        //    }
        params
    }
}
