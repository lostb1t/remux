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
pub struct TraktStatsResponse {
    pub watchers: u64,
}

#[derive(Debug, Clone)]
pub struct MovieStatsEndpoint {
    pub imdb_id: String,
}

impl Endpoint for MovieStatsEndpoint {
    type Output = TraktStatsResponse;

    fn path(&self) -> String {
        format!("movies/{}/stats", self.imdb_id)
    }
}

#[derive(Debug, Clone)]
pub struct ShowStatsEndpoint {
    pub imdb_id: String,
}

impl Endpoint for ShowStatsEndpoint {
    type Output = TraktStatsResponse;

    fn path(&self) -> String {
        format!("shows/{}/stats", self.imdb_id)
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
