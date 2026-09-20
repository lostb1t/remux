use chrono::{DateTime, Utc};
use http::StatusCode;
use remux_utils::Secret;
use reqwest::{Method, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const DEVICE_GRANT: &str = "urn:ietf:params:oauth:grant-type:device_code";

#[derive(Debug, thiserror::Error)]
pub enum SimklError {
    #[error("Simkl authorization is invalid or expired")]
    Unauthorized,
    #[error("Simkl rate limit reached")]
    RateLimited { retry_after: Option<Duration> },
    #[error("Simkl request failed with status {status}: {message}")]
    Http { status: u16, message: String },
    #[error("Simkl custom-list access requires Simkl PRO: {message}")]
    PremiumOnly { message: String },
    #[error("Simkl request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

impl SimklError {
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            Self::RateLimited { .. }
                | Self::Http {
                    status: 500..=599,
                    ..
                }
                | Self::Transport(_)
        )
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimklDeviceCode {
    pub device_code: Secret<String>,
    pub user_code: String,
    pub verification_uri: String,
    pub verification_uri_complete: Option<String>,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimklToken {
    pub access_token: Secret<String>,
    pub refresh_token: Secret<String>,
    pub expires_in: i64,
    pub token_type: String,
    pub scope: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimklCredentials {
    pub access_token: Secret<String>,
    pub refresh_token: Secret<String>,
    pub expires_at: DateTime<Utc>,
}

impl From<SimklToken> for SimklCredentials {
    fn from(value: SimklToken) -> Self {
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at: Utc::now() + chrono::Duration::seconds(value.expires_in),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Pending,
    SlowDown,
    Expired,
}

#[derive(Debug)]
pub enum DeviceTokenResponse {
    Approved(SimklToken),
    Waiting(DeviceTokenPoll),
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SimklItemIds {
    pub simkl: Option<i64>,
    pub simkl_id: Option<i64>,
    pub imdb: Option<String>,
    pub tmdb: Option<String>,
    pub tvdb: Option<String>,
    pub kitsu: Option<String>,
    pub mal: Option<String>,
    pub anidb: Option<String>,
    pub anilist: Option<String>,
}

impl SimklItemIds {
    pub fn simkl_id(&self) -> Option<i64> {
        self.simkl
            .or(self.simkl_id)
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklMedia {
    pub title: String,
    pub year: Option<i32>,
    pub ids: SimklItemIds,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SimklStatus {
    Watching,
    Plantowatch,
    Hold,
    Completed,
    Dropped,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SimklMediaType {
    Movie,
    Tv,
    Anime,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SimklCustomListMediaType {
    Movies,
    Tv,
    Anime,
    Manga,
    Characters,
    Companies,
}

impl SimklCustomListMediaType {
    pub fn is_video(self) -> bool {
        matches!(self, Self::Movies | Self::Tv | Self::Anime)
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Deserialize,
    Serialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum SimklAnimeType {
    Tv,
    Special,
    Ova,
    Movie,
    #[serde(rename = "music video")]
    #[strum(serialize = "music video")]
    MusicVideo,
    Ona,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklEpisodeCoordinate {
    pub season: i64,
    pub episode: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklWatchedEpisode {
    pub number: i64,
    pub watched_at: Option<DateTime<Utc>>,
    pub tvdb: Option<SimklEpisodeCoordinate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklWatchedSeason {
    pub number: i64,
    #[serde(default)]
    pub episodes: Vec<SimklWatchedEpisode>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklLibraryEntry {
    pub last_watched_at: Option<DateTime<Utc>>,
    pub status: SimklStatus,
    pub user_rating: Option<i32>,
    pub show: Option<SimklMedia>,
    pub movie: Option<SimklMedia>,
    pub anime_type: Option<SimklAnimeType>,
    #[serde(default)]
    pub seasons: Vec<SimklWatchedSeason>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SimklLibrary {
    pub shows: Vec<SimklLibraryEntry>,
    pub movies: Vec<SimklLibraryEntry>,
    pub anime: Vec<SimklLibraryEntry>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklPagination {
    pub page: u32,
    pub limit: u32,
    pub total_items: u32,
    pub total_pages: u32,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct SimklCustomListCounts {
    pub items: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklCustomList {
    pub id: i64,
    pub name: String,
    pub media_type: SimklCustomListMediaType,
    #[serde(default)]
    pub counts: SimklCustomListCounts,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklCustomListsPage {
    pub pagination: SimklPagination,
    #[serde(default)]
    pub lists: Vec<SimklCustomList>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklCustomListItem {
    pub title: String,
    pub year: Option<i32>,
    #[serde(rename = "type")]
    pub kind: SimklMediaType,
    pub ids: SimklItemIds,
    pub anime_type: Option<SimklAnimeType>,
    pub release_date: Option<DateTime<Utc>>,
    pub overview: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklCustomListPage {
    pub pagination: SimklPagination,
    #[serde(default)]
    pub items: Vec<SimklCustomListItem>,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum SimklCustomListPageResponse {
    Page(SimklCustomListPage),
    Error(SimklBodyError),
}

#[derive(Debug, Deserialize)]
struct SimklBodyError {
    error: String,
    message: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklEpisodeDetail {
    pub title: String,
    pub description: Option<String>,
    pub season: Option<i64>,
    pub episode: Option<i64>,
    #[serde(rename = "type")]
    pub kind: String,
    pub aired: Option<bool>,
    pub date: Option<DateTime<Utc>>,
    pub ids: SimklItemIds,
    pub tvdb: Option<SimklEpisodeCoordinate>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklSettingsUser {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklSettingsAccount {
    pub id: i64,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SimklSettings {
    pub user: SimklSettingsUser,
    pub account: SimklSettingsAccount,
}

#[derive(Debug, Clone)]
pub struct SimklClient {
    http: reqwest::Client,
    base_url: String,
    client_id: String,
}

impl SimklClient {
    pub fn new(base_url: impl Into<String>, client_id: impl Into<String>) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url
                .into()
                .trim_end_matches('/')
                .to_string(),
            client_id: client_id.into(),
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .query(&[
                (
                    "client_id",
                    self.client_id
                        .as_str(),
                ),
                ("app-name", "remux"),
                ("app-version", env!("CARGO_PKG_VERSION")),
            ])
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                format!("remux-server/{}", env!("CARGO_PKG_VERSION")),
            )
    }

    fn authenticated(
        &self,
        method: Method,
        path: &str,
        credentials: &SimklCredentials,
    ) -> reqwest::RequestBuilder {
        self.request(method, path)
            .bearer_auth(
                credentials
                    .access_token
                    .expose(),
            )
    }

    pub async fn begin_device_auth(&self) -> Result<SimklDeviceCode, SimklError> {
        self.send_json(
            self.request(Method::POST, "/oauth2/device")
                .form(&[
                    (
                        "client_id",
                        self.client_id
                            .as_str(),
                    ),
                    ("scope", "media:read"),
                ]),
        )
        .await
    }

    pub async fn poll_device_token(
        &self,
        device_code: &str,
    ) -> Result<DeviceTokenResponse, SimklError> {
        let response = self
            .request(Method::POST, "/oauth2/token")
            .form(&[
                ("grant_type", DEVICE_GRANT),
                (
                    "client_id",
                    self.client_id
                        .as_str(),
                ),
                ("device_code", device_code),
            ])
            .send()
            .await?;
        if response
            .status()
            .is_success()
        {
            return Ok(DeviceTokenResponse::Approved(
                response
                    .json()
                    .await?,
            ));
        }
        if response.status() == StatusCode::BAD_REQUEST {
            #[derive(Deserialize)]
            struct OAuthError {
                error: String,
            }
            let status = response
                .status()
                .as_u16();
            let body = response
                .text()
                .await?;
            if let Ok(error) = serde_json::from_str::<OAuthError>(&body) {
                return match error
                    .error
                    .as_str()
                {
                    "authorization_pending" => {
                        Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::Pending))
                    }
                    "slow_down" => {
                        Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::SlowDown))
                    }
                    "expired_token" => {
                        Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::Expired))
                    }
                    _ => Err(SimklError::Http {
                        status,
                        message: body,
                    }),
                };
            }
            return Err(SimklError::Http {
                status,
                message: body,
            });
        }
        Err(Self::response_error(response).await)
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
    ) -> Result<SimklCredentials, SimklError> {
        let token: SimklToken = self
            .send_json(
                self.request(Method::POST, "/oauth2/token")
                    .form(&[
                        ("grant_type", "refresh_token"),
                        (
                            "client_id",
                            self.client_id
                                .as_str(),
                        ),
                        ("refresh_token", refresh_token),
                    ]),
            )
            .await?;
        Ok(token.into())
    }

    pub async fn revoke(&self, refresh_token: &str) -> Result<(), SimklError> {
        self.send_empty(
            self.request(Method::POST, "/oauth2/revoke")
                .form(&[
                    (
                        "client_id",
                        self.client_id
                            .as_str(),
                    ),
                    ("token", refresh_token),
                ]),
        )
        .await
    }

    pub async fn settings(
        &self,
        credentials: &SimklCredentials,
    ) -> Result<SimklSettings, SimklError> {
        self.send_json(self.authenticated(Method::GET, "/users/settings", credentials))
            .await
    }

    pub async fn library(
        &self,
        credentials: &SimklCredentials,
    ) -> Result<SimklLibrary, SimklError> {
        self.send_json(self.authenticated(
            Method::GET,
            "/sync/all-items?extended=full_anime_seasons&include_all_episodes=yes&episode_watched_at=yes",
            credentials,
        ))
        .await
    }

    pub async fn library_by_status(
        &self,
        credentials: &SimklCredentials,
        status: SimklStatus,
    ) -> Result<SimklLibrary, SimklError> {
        self.send_json(self.authenticated(
            Method::GET,
            &format!("/sync/all-items/all/{status}"),
            credentials,
        ))
        .await
    }

    pub async fn custom_lists(
        &self,
        credentials: &SimklCredentials,
        user_id: i64,
    ) -> Result<Vec<SimklCustomList>, SimklError> {
        const PAGE_SIZE: u32 = 500;

        let mut page = 1u32;
        let mut lists = Vec::new();
        loop {
            let response: SimklCustomListsPage = self
                .send_json(
                    self.authenticated(
                        Method::GET,
                        &format!("/lists/user/{user_id}"),
                        credentials,
                    )
                    .query(&[
                        ("page", page.to_string()),
                        ("limit", PAGE_SIZE.to_string()),
                        ("followed", "true".to_string()),
                        ("collaborants", "true".to_string()),
                    ]),
                )
                .await?;
            let last_page = response
                .pagination
                .total_pages
                .max(1);
            lists.extend(response.lists);
            if page >= last_page {
                break;
            }
            page += 1;
        }
        Ok(lists)
    }

    pub async fn custom_list_items(
        &self,
        credentials: &SimklCredentials,
        list_id: i64,
    ) -> Result<Vec<SimklCustomListItem>, SimklError> {
        const PAGE_SIZE: u32 = 500;

        let mut page = 1u32;
        let mut items = Vec::new();
        loop {
            let response: SimklCustomListPageResponse = self
                .send_json(
                    self.authenticated(
                        Method::GET,
                        &format!("/lists/{list_id}"),
                        credentials,
                    )
                    .query(&[
                        ("page", page.to_string()),
                        ("limit", PAGE_SIZE.to_string()),
                    ]),
                )
                .await?;
            let response = match response {
                SimklCustomListPageResponse::Page(response) => response,
                SimklCustomListPageResponse::Error(error)
                    if error.error == "premium_only" =>
                {
                    return Err(SimklError::PremiumOnly {
                        message: error.message,
                    });
                }
                SimklCustomListPageResponse::Error(error) => {
                    return Err(SimklError::Http {
                        status: 200,
                        message: format!("{}: {}", error.error, error.message),
                    });
                }
            };
            let last_page = response
                .pagination
                .total_pages
                .max(1);
            items.extend(response.items);
            if page >= last_page {
                break;
            }
            page += 1;
        }
        Ok(items)
    }

    pub async fn episodes(
        &self,
        simkl_id: i64,
        anime: bool,
    ) -> Result<Vec<SimklEpisodeDetail>, SimklError> {
        let category = if anime { "anime" } else { "tv" };
        self.send_json(
            self.request(Method::GET, &format!("/{category}/episodes/{simkl_id}")),
        )
        .await
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, SimklError> {
        let response = request
            .send()
            .await?;
        if !response
            .status()
            .is_success()
        {
            return Err(Self::response_error(response).await);
        }
        Ok(response
            .json()
            .await?)
    }

    async fn send_empty(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<(), SimklError> {
        let response = request
            .send()
            .await?;
        if response
            .status()
            .is_success()
        {
            return Ok(());
        }
        Err(Self::response_error(response).await)
    }

    async fn response_error(response: Response) -> SimklError {
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return SimklError::Unauthorized;
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|value| {
                    value
                        .to_str()
                        .ok()
                })
                .and_then(|value| {
                    value
                        .parse::<u64>()
                        .ok()
                })
                .map(Duration::from_secs);
            return SimklError::RateLimited { retry_after };
        }
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| "request failed".to_string());
        SimklError::Http {
            status: status.as_u16(),
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_both_simkl_id_spellings() {
        let canonical: SimklItemIds = serde_json::from_value(serde_json::json!({
            "simkl": 42,
            "tmdb": "100"
        }))
        .unwrap();
        let episode: SimklItemIds = serde_json::from_value(serde_json::json!({
            "simkl_id": 43
        }))
        .unwrap();

        assert_eq!(canonical.simkl_id(), Some(42));
        assert_eq!(episode.simkl_id(), Some(43));
        assert_eq!(
            canonical
                .tmdb
                .as_deref(),
            Some("100")
        );
    }

    #[test]
    fn decodes_anime_tvdb_episode_coordinates() {
        let episode: SimklEpisodeDetail = serde_json::from_value(serde_json::json!({
            "title": "Cruelty",
            "episode": 1,
            "type": "episode",
            "aired": true,
            "date": "2019-04-06T23:30:00+09:00",
            "ids": { "simkl_id": 4170591 },
            "tvdb": { "season": 1, "episode": 1 }
        }))
        .unwrap();

        assert_eq!(
            episode
                .tvdb
                .unwrap()
                .season,
            1
        );
        assert_eq!(
            episode
                .ids
                .simkl_id(),
            Some(4_170_591)
        );
    }

    #[test]
    fn decodes_custom_list_catalogs() {
        let page: SimklCustomListsPage = serde_json::from_value(serde_json::json!({
            "pagination": { "page": 1, "limit": 500, "total_items": 2, "total_pages": 1 },
            "lists": [
                { "id": 7, "name": "Favorites", "media_type": "movies", "counts": { "items": 12 } },
                { "id": 8, "name": "People", "media_type": "characters", "counts": { "items": 3 } }
            ]
        }))
        .unwrap();

        assert_eq!(page.lists[0].media_type, SimklCustomListMediaType::Movies);
        assert!(
            page.lists[0]
                .media_type
                .is_video()
        );
        assert!(
            !page.lists[1]
                .media_type
                .is_video()
        );
    }

    #[test]
    fn decodes_custom_list_items_and_premium_response() {
        let page: SimklCustomListPageResponse = serde_json::from_value(serde_json::json!({
            "pagination": { "page": 1, "limit": 500, "total_items": 1, "total_pages": 1 },
            "items": [{
                "title": "Akira",
                "year": 1988,
                "type": "anime",
                "anime_type": "movie",
                "ids": { "simkl": 123 }
            }]
        }))
        .unwrap();
        let SimklCustomListPageResponse::Page(page) = page else {
            panic!("expected a custom-list page");
        };
        assert_eq!(page.items[0].kind, SimklMediaType::Anime);
        assert_eq!(page.items[0].anime_type, Some(SimklAnimeType::Movie));

        let premium: SimklCustomListPageResponse =
            serde_json::from_value(serde_json::json!({
                "error": "premium_only",
                "message": "Upgrade to access this endpoint"
            }))
            .unwrap();
        assert!(matches!(
            premium,
            SimklCustomListPageResponse::Error(SimklBodyError { error, .. })
                if error == "premium_only"
        ));
    }
}
