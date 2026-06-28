use crate::{Auth, Endpoint, RestClient};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct TraktAuth {
    pub client_id: String,
}

impl Auth for TraktAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("trakt-api-key", &self.client_id)
            .header("trakt-api-version", "2")
            .header("Content-Type", "application/json")
            .header("Accept", "application/json")
            .header("User-Agent", "Mozilla/5.0 (compatible; remux/1.0)")
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraktItemIds {
    pub imdb: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraktPopularItem {
    pub ids: TraktItemIds,
}

#[derive(Debug, Clone, Serialize)]
pub struct PopularParams {
    pub limit: u32,
}

#[derive(Debug, Clone)]
pub struct MoviePopularEndpoint {
    pub limit: u32,
}

impl Endpoint for MoviePopularEndpoint {
    type Output = Vec<TraktPopularItem>;

    fn path(&self) -> String {
        "movies/popular".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        PopularParams { limit: self.limit }
    }
}

#[derive(Debug, Clone)]
pub struct ShowPopularEndpoint {
    pub limit: u32,
}

impl Endpoint for ShowPopularEndpoint {
    type Output = Vec<TraktPopularItem>;

    fn path(&self) -> String {
        "shows/popular".to_string()
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        PopularParams { limit: self.limit }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct TraktStats {
    pub watchers: u64,
    pub recommended: u64,
    pub favorited: u64,
}

impl TraktStats {
    pub fn raw_score(&self) -> f64 {
        self.watchers as f64
            + self.recommended as f64 * 20.0
            + self.favorited as f64 * 10.0
    }
}

#[derive(Debug, Clone)]
pub struct MovieStatsEndpoint {
    pub imdb_id: String,
}

impl Endpoint for MovieStatsEndpoint {
    type Output = TraktStats;

    fn path(&self) -> String {
        format!("movies/{}/stats", self.imdb_id)
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        ()
    }
}

#[derive(Debug, Clone)]
pub struct ShowStatsEndpoint {
    pub imdb_id: String,
}

impl Endpoint for ShowStatsEndpoint {
    type Output = TraktStats;

    fn path(&self) -> String {
        format!("shows/{}/stats", self.imdb_id)
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        ()
    }
}

pub fn trakt_client(
    client_id: &str,
    base_url: &str,
) -> Result<RestClient<TraktAuth>, url::ParseError> {
    Ok(RestClient::new(base_url)?.with_auth(TraktAuth {
        client_id: client_id.to_string(),
    }))
}
