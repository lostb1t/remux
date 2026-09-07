//! TheTVDB v4, for the one thing TMDB and Cinemeta between them cannot always
//! answer: the tvdb id of a specific episode.
//!
//! Deliberately narrow. remux already knows a series' tvdb id, so the only
//! call worth making is series plus season and episode number, which TVDB
//! answers in one request rather than an enumeration.

use crate::{Body, Endpoint, NoAuth, RestClient};
use http::Method;
use serde::{Deserialize, Serialize};

pub const BASE_URL: &str = "https://api4.thetvdb.com/v4/";

/// Which numbering a season and episode number are expressed in. `Default` is
/// whatever the series itself declares; `Absolute` is the one that addresses a
/// long-running show by a single running count.
#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SeasonType {
    #[default]
    Default,
    Official,
    Dvd,
    Absolute,
    Alternate,
    Regional,
}

/// `POST /login`. The token it returns is good for a month and there is no
/// refresh endpoint, so a caller that has kept one past its life logs in again.
#[derive(Debug, Clone)]
pub struct LoginEndpoint {
    pub api_key: String,
    /// Only a user-supported key carries one. TVDB rejects the call if a
    /// project key sends `pin` at all, so it is omitted rather than sent empty.
    pub pin: Option<String>,
}

impl Endpoint for LoginEndpoint {
    type Output = LoginResponse;

    fn path(&self) -> String {
        "login".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn body(&self) -> Body {
        let mut map = serde_json::Map::new();
        map.insert(
            "apikey".into(),
            serde_json::Value::String(
                self.api_key
                    .clone(),
            ),
        );
        if let Some(pin) = self
            .pin
            .as_ref()
            .filter(|p| !p.is_empty())
        {
            map.insert("pin".into(), serde_json::Value::String(pin.clone()));
        }
        Body::Json(serde_json::Value::Object(map))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub data: LoginData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginData {
    pub token: String,
}

/// `GET /series/{id}/episodes/{season_type}`, filtered to one episode.
///
/// TVDB requires `season` alongside `episode_number`; it answers 400 for an
/// episode number on its own.
#[derive(Debug, Clone)]
pub struct SeriesEpisodesEndpoint {
    pub series_id: i64,
    pub season_type: SeasonType,
    pub season: Option<i64>,
    pub episode_number: Option<i64>,
}

impl Endpoint for SeriesEpisodesEndpoint {
    type Output = EpisodesResponse;

    fn path(&self) -> String {
        format!("series/{}/episodes/{}", self.series_id, self.season_type)
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut q = vec![("page".to_string(), "0".to_string())];
        if let Some(s) = self.season {
            q.push(("season".to_string(), s.to_string()));
        }
        if let Some(e) = self.episode_number {
            q.push(("episodeNumber".to_string(), e.to_string()));
        }
        q
    }
}

/// The filtered call still answers with a list, so a miss is an empty one
/// rather than a 404.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodesResponse {
    pub data: EpisodesData,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpisodesData {
    #[serde(default)]
    pub episodes: Vec<EpisodeRecord>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EpisodeRecord {
    pub id: i64,
    pub season_number: Option<i64>,
    pub number: Option<i64>,
    pub absolute_number: Option<i64>,
}

impl EpisodesResponse {
    /// The id of the single episode a filtered call was for, if it matched one.
    pub fn episode_id(&self) -> Option<i64> {
        self.data
            .episodes
            .first()
            .map(|e| e.id)
    }
}

/// Unauthenticated, for `/login` alone. Every other call needs the bearer
/// token that returns.
pub fn client(base_url: &str) -> Result<RestClient<NoAuth>, url::ParseError> {
    RestClient::new(base_url).map(|c| {
        c.with_retry(crate::ExponentialBackoff::builder().build_with_max_retries(3))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body_json(ep: &LoginEndpoint) -> serde_json::Value {
        match ep.body() {
            Body::Json(v) => v,
            _ => panic!("login posts json"),
        }
    }

    /// TVDB rejects a login that carries `pin` alongside a project key, so the
    /// field has to be absent rather than null or empty.
    #[test]
    fn a_project_key_logs_in_without_a_pin() {
        let body = body_json(&LoginEndpoint {
            api_key: "k".into(),
            pin: None,
        });
        assert_eq!(body["apikey"], "k");
        assert!(
            body.get("pin")
                .is_none(),
            "pin must not be sent at all: {body}"
        );
    }

    #[test]
    fn a_user_supported_key_carries_its_pin() {
        let body = body_json(&LoginEndpoint {
            api_key: "k".into(),
            pin: Some("1234".into()),
        });
        assert_eq!(body["pin"], "1234");
    }

    /// An empty pin is the same as none. A settings field left blank would
    /// otherwise turn a working project key into a rejected login.
    #[test]
    fn a_blank_pin_is_not_sent() {
        let body = body_json(&LoginEndpoint {
            api_key: "k".into(),
            pin: Some(String::new()),
        });
        assert!(
            body.get("pin")
                .is_none()
        );
    }

    /// TVDB answers 400 for an episode number without a season, so both go on
    /// together or the call is not worth making.
    #[test]
    fn an_episode_lookup_asks_for_one_season_and_number() {
        let ep = SeriesEpisodesEndpoint {
            series_id: 76184,
            season_type: SeasonType::Default,
            season: Some(0),
            episode_number: Some(1),
        };
        assert_eq!(ep.path(), "series/76184/episodes/default");
        let q = ep.query();
        assert!(q.contains(&("page".to_string(), "0".to_string())));
        assert!(q.contains(&("season".to_string(), "0".to_string())));
        assert!(q.contains(&("episodeNumber".to_string(), "1".to_string())));
    }

    #[test]
    fn the_season_type_is_named_as_tvdb_spells_it() {
        assert_eq!(
            SeriesEpisodesEndpoint {
                series_id: 1,
                season_type: SeasonType::Absolute,
                season: None,
                episode_number: None,
            }
            .path(),
            "series/1/episodes/absolute"
        );
    }

    /// The filtered call answers with a list either way, so a miss is an empty
    /// one rather than a 404.
    #[test]
    fn a_match_and_a_miss_are_both_a_list() {
        let hit: EpisodesResponse = serde_json::from_value(serde_json::json!({
            "data": { "episodes": [{
                "id": 5711666, "seasonNumber": 0, "number": 1, "absoluteNumber": null
            }] }
        }))
        .unwrap();
        assert_eq!(hit.episode_id(), Some(5711666));

        let miss: EpisodesResponse =
            serde_json::from_value(serde_json::json!({ "data": { "episodes": [] } }))
                .unwrap();
        assert_eq!(miss.episode_id(), None);
    }
}
