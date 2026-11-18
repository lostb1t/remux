use serde::{Deserialize, Serialize, Serializer};
use serde_with::{VecSkipError, serde_as, skip_serializing_none};

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
    pub videos: Option<super::Videos>,
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
    pub include_image_language: Option<String>,
    pub include_video_language: Option<String>,
    #[serde(
        skip_serializing_if = "Vec::is_empty",
        serialize_with = "serialize_comma"
    )]
    pub append_to_response: Vec<String>,
}

impl SeriesEndpoint {
    pub fn new(id: i64, language: Option<String>) -> Self {
        Self {
            id,
            language,
            include_image_language: None,
            include_video_language: None,
            append_to_response: default_append_to_response(),
        }
    }

    pub fn with_image_languages(mut self, languages: impl Into<String>) -> Self {
        self.include_image_language = Some(languages.into());
        self
    }

    /// Also fetches the videos in `languages`; without it TMDB only returns
    /// the ones in the request's `language`.
    pub fn with_videos(mut self, languages: impl Into<String>) -> Self {
        self.append_to_response
            .push("videos".to_string());
        self.include_video_language = Some(languages.into());
        self
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
    pub external_ids: Option<super::ExternalIds>,
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

#[serde_as]
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
    #[serde_as(as = "Option<VecSkipError<_>>")]
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

    pub include_image_language: Option<String>,

    //#[builder(default = "Some(vec![\"images\".to_string(), \"external_ids\".to_string()])")]
    #[serde(serialize_with = "serialize_comma_opt")]
    pub append_to_response: Option<Vec<String>>,
}

impl EpisodeEndpoint {
    pub fn new(
        series_id: i64,
        season_number: i64,
        episode_number: i64,
        language: Option<String>,
    ) -> Self {
        Self {
            series_id,
            season_number,
            episode_number,
            language,
            include_image_language: None,
            append_to_response: Some(super::default_append_to_response()),
        }
    }

    pub fn with_image_languages(mut self, languages: impl Into<String>) -> Self {
        self.include_image_language = Some(languages.into());
        self
    }
}

impl Endpoint for EpisodeEndpoint {
    type Output = Option<Episode>;

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
    pub popularity: Option<f64>,
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

#[derive(Debug, Clone)]
pub struct SeasonExternalIdsEndpoint {
    pub series_id: i64,
    pub season_number: i64,
}

impl Endpoint for SeasonExternalIdsEndpoint {
    type Output = super::ExternalIds;

    fn path(&self) -> String {
        format!(
            "tv/{}/season/{}/external_ids",
            self.series_id, self.season_number
        )
    }
}

#[derive(Debug, Clone)]
pub struct EpisodeExternalIdsEndpoint {
    pub series_id: i64,
    pub season_number: i64,
    pub episode_number: i64,
}

impl Endpoint for EpisodeExternalIdsEndpoint {
    type Output = super::ExternalIds;

    fn path(&self) -> String {
        format!(
            "tv/{}/season/{}/episode/{}/external_ids",
            self.series_id, self.season_number, self.episode_number
        )
    }
}

/// Most `append_to_response` sub-requests TMDB answers in one call; with one
/// more it rejects the whole request.
pub const MAX_APPENDS: usize = 20;

/// The external ids of up to [`MAX_APPENDS`] episodes of one season in a
/// single call. TMDB's season listing carries no ids per episode, and asking
/// for them one episode at a time costs a call per episode. An episode TMDB
/// doesn't have is left out of the answer rather than failing it.
#[derive(Debug, Clone)]
pub struct EpisodeExternalIdsBatchEndpoint {
    pub series_id: i64,
    pub season_number: i64,
    pub episode_numbers: Vec<i64>,
}

impl Endpoint for EpisodeExternalIdsBatchEndpoint {
    type Output = EpisodeExternalIdsBatch;

    fn path(&self) -> String {
        format!("tv/{}", self.series_id)
    }

    fn query_params(&self) -> impl serde::Serialize + '_ {
        let appends = self
            .episode_numbers
            .iter()
            .map(|e| format!("season/{}/episode/{e}/external_ids", self.season_number))
            .collect::<Vec<_>>()
            .join(",");
        [("append_to_response", appends)]
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(from = "std::collections::HashMap<String, serde_json::Value>")]
pub struct EpisodeExternalIdsBatch {
    pub episodes: Vec<EpisodeExternalIds>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EpisodeExternalIds {
    pub season_number: i64,
    pub episode_number: i64,
    pub ids: super::ExternalIds,
}

impl EpisodeExternalIdsBatch {
    pub fn get(
        &self,
        season_number: i64,
        episode_number: i64,
    ) -> Option<&super::ExternalIds> {
        self.episodes
            .iter()
            .find(|e| {
                e.season_number == season_number && e.episode_number == episode_number
            })
            .map(|e| &e.ids)
    }
}

/// Keeps only the appended `season/S/episode/E/external_ids` entries of the
/// series response, not the series itself.
impl From<std::collections::HashMap<String, serde_json::Value>>
    for EpisodeExternalIdsBatch
{
    fn from(response: std::collections::HashMap<String, serde_json::Value>) -> Self {
        let episodes = response
            .into_iter()
            .filter_map(|(key, value)| {
                let rest = key.strip_prefix("season/")?;
                let (season, rest) = rest.split_once("/episode/")?;
                let episode = rest.strip_suffix("/external_ids")?;
                Some(EpisodeExternalIds {
                    season_number: season
                        .parse()
                        .ok()?,
                    episode_number: episode
                        .parse()
                        .ok()?,
                    ids: serde_json::from_value(value).ok()?,
                })
            })
            .collect();
        Self { episodes }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episode_id_batches_keep_only_the_appended_episodes() {
        let endpoint = EpisodeExternalIdsBatchEndpoint {
            series_id: 57532,
            season_number: 2,
            episode_numbers: vec![1, 2],
        };
        assert_eq!(endpoint.path(), "tv/57532");
        let query = serde_json::to_value(endpoint.query_params()).unwrap();
        assert_eq!(
            query[0][1],
            "season/2/episode/1/external_ids,season/2/episode/2/external_ids"
        );

        let batch: EpisodeExternalIdsBatch = serde_json::from_value(serde_json::json!({
            "id": 57532,
            "name": "PAW Patrol",
            "external_ids": {"tvdb_id": 1},
            "season/2/episode/1/external_ids": {"imdb_id": null, "tvdb_id": 4963614},
            "season/2/episode/2/external_ids": {"imdb_id": "tt1", "tvdb_id": null}
        }))
        .unwrap();
        assert_eq!(
            batch
                .episodes
                .len(),
            2
        );
        assert_eq!(
            batch
                .get(2, 1)
                .and_then(|i| i.tvdb_id),
            Some(4963614)
        );
        assert_eq!(
            batch
                .get(2, 2)
                .and_then(|i| i
                    .imdb_id
                    .as_deref()),
            Some("tt1")
        );
        assert!(
            batch
                .get(2, 3)
                .is_none()
        );
    }

    #[test]
    fn external_id_endpoints_address_the_child_not_the_series() {
        assert_eq!(
            SeasonExternalIdsEndpoint {
                series_id: 10,
                season_number: 2
            }
            .path(),
            "tv/10/season/2/external_ids"
        );
        assert_eq!(
            EpisodeExternalIdsEndpoint {
                series_id: 10,
                season_number: 2,
                episode_number: 3
            }
            .path(),
            "tv/10/season/2/episode/3/external_ids"
        );
    }

    #[test]
    fn season_external_ids_are_optional_and_deserialized() {
        let mut value =
            serde_json::json!({"id": 20, "name": "Season 2", "season_number": 2});
        let season: Season = serde_json::from_value(value.clone()).unwrap();
        assert!(
            season
                .external_ids
                .is_none()
        );
        value["external_ids"] = serde_json::json!({"tvdb_id": 456});
        let season: Season = serde_json::from_value(value).unwrap();
        let ids = season
            .external_ids
            .unwrap();
        assert_eq!(ids.tvdb_id, Some(456));
        assert!(
            ids.imdb_id
                .is_none()
        );
    }
}
