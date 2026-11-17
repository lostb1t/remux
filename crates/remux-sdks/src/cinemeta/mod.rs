//! Cinemeta, Stremio's own metadata addon, as the source of TVDB episode
//! numbering.
//!
//! The Stremio ecosystem names an episode `{series imdb id}:{season}:{episode}`
//! with the season and episode in TVDB's aired order, which is what release
//! names follow. Nearly every stream addon (Torrentio, Comet, ...) only
//! accepts those ids. Cinemeta lists every episode of a series with its TVDB
//! id under exactly that numbering, so it maps a TVDB episode to the number
//! those addons expect.

use crate::{Endpoint, NoAuth, RestClient};
use serde::{Deserialize, Serialize};

pub const DEFAULT_BASE_URL: &str = "https://v3-cinemeta.strem.io/";

/// Every episode of a series, by the series' IMDb id.
#[derive(Debug, Clone)]
pub struct SeriesEpisodesEndpoint {
    pub imdb_id: String,
}

impl Endpoint for SeriesEpisodesEndpoint {
    type Output = SeriesEpisodes;

    fn path(&self) -> String {
        format!("meta/series/{}.json", self.imdb_id)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "MetaResponse")]
pub struct SeriesEpisodes {
    pub episodes: Vec<Episode>,
}

/// An episode's place in TVDB's aired order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    pub tvdb_id: i64,
    pub season: i64,
    pub episode: i64,
}

impl SeriesEpisodes {
    pub fn by_tvdb_id(&self, tvdb_id: i64) -> Option<&Episode> {
        self.episodes
            .iter()
            .find(|e| e.tvdb_id == tvdb_id)
    }
}

#[derive(Deserialize)]
struct MetaResponse {
    #[serde(default)]
    meta: Option<Meta>,
}

#[derive(Deserialize)]
struct Meta {
    #[serde(default)]
    videos: Vec<serde_json::Value>,
}

/// Cinemeta sends the number as both `episode` and `number`, older entries
/// only the latter.
#[derive(Deserialize)]
struct Video {
    tvdb_id: i64,
    season: i64,
    episode: Option<i64>,
    number: Option<i64>,
}

/// Keeps the videos that carry a TVDB id and both numbers, and drops any one
/// that doesn't parse rather than failing the series.
impl From<MetaResponse> for SeriesEpisodes {
    fn from(response: MetaResponse) -> Self {
        let episodes = response
            .meta
            .map(|m| m.videos)
            .unwrap_or_default()
            .into_iter()
            .filter_map(|v| serde_json::from_value::<Video>(v).ok())
            .filter_map(|v| {
                Some(Episode {
                    tvdb_id: v.tvdb_id,
                    season: v.season,
                    episode: v
                        .episode
                        .or(v.number)?,
                })
            })
            .collect();
        Self { episodes }
    }
}

pub fn client(base: &str) -> Result<RestClient<NoAuth>, url::ParseError> {
    Ok(RestClient::new(base)?
        .with_retry(crate::ExponentialBackoff::builder().build_with_max_retries(3)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_are_keyed_by_their_tvdb_id() {
        let endpoint = SeriesEpisodesEndpoint {
            imdb_id: "tt3121722".to_string(),
        };
        assert_eq!(endpoint.path(), "meta/series/tt3121722.json");

        let series: SeriesEpisodes = serde_json::from_value(serde_json::json!({"meta": {
            "id": "tt3121722",
            "videos": [
                {"id": "tt3121722:2:9", "name": "The New Pup", "season": 2, "episode": 9, "number": 9, "tvdb_id": 5050934},
                {"id": "tt3121722:0:1", "season": 0, "number": 1, "tvdb_id": 1},
                {"id": "tt3121722:1:1", "season": 1, "episode": 1},
                {"id": "broken", "season": "x", "episode": 1, "tvdb_id": 2}
            ]
        }}))
        .unwrap();
        assert_eq!(
            series
                .episodes
                .len(),
            2
        );
        assert_eq!(
            series.by_tvdb_id(5050934),
            Some(&Episode {
                tvdb_id: 5050934,
                season: 2,
                episode: 9
            })
        );
        assert_eq!(
            series
                .by_tvdb_id(1)
                .map(|e| e.episode),
            Some(1)
        );
        assert!(
            series
                .by_tvdb_id(2)
                .is_none()
        );
    }

    #[test]
    fn a_series_cinemeta_does_not_know_has_no_episodes() {
        let series: SeriesEpisodes =
            serde_json::from_value(serde_json::json!({})).unwrap();
        assert!(
            series
                .episodes
                .is_empty()
        );
    }
}
