use crate::{Auth, Endpoint, RestClient};
use chrono::{DateTime, Utc};
use http::StatusCode;
use remux_utils::Secret;
use reqwest::{Method, Response};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::time::Duration;

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

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct TraktItemIds {
    pub imdb: Option<String>,
    pub tmdb: Option<i64>,
    pub tvdb: Option<i64>,
    pub trakt: Option<i64>,
    pub slug: Option<String>,
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
    Ok(RestClient::new(base_url)?
        .with_auth(TraktAuth {
            client_id: client_id.to_string(),
        })
        .with_retry(crate::ExponentialBackoff::builder().build_with_max_retries(3)))
}

#[derive(Debug, thiserror::Error)]
pub enum TraktError {
    #[error("trakt authorization is invalid or expired")]
    Unauthorized,
    #[error("trakt rate limit reached")]
    RateLimited { retry_after: Option<Duration> },
    #[error("trakt request failed with status {status}: {message}")]
    Http { status: u16, message: String },
    #[error("trakt request failed: {0}")]
    Transport(#[from] reqwest::Error),
}

impl TraktError {
    pub fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::RateLimited { retry_after } => *retry_after,
            _ => None,
        }
    }

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
pub struct TraktDeviceCode {
    pub device_code: Secret<String>,
    pub user_code: String,
    pub verification_url: String,
    pub expires_in: u64,
    pub interval: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktToken {
    pub access_token: Secret<String>,
    pub refresh_token: Secret<String>,
    pub expires_in: i64,
    pub created_at: i64,
    pub token_type: String,
    pub scope: String,
}

impl TraktToken {
    pub fn expires_at(&self) -> DateTime<Utc> {
        DateTime::from_timestamp(
            self.created_at
                .saturating_add(self.expires_in),
            0,
        )
        .unwrap_or_else(Utc::now)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktCredentials {
    pub access_token: Secret<String>,
    pub refresh_token: Secret<String>,
    pub expires_at: DateTime<Utc>,
}

impl From<TraktToken> for TraktCredentials {
    fn from(value: TraktToken) -> Self {
        let expires_at = value.expires_at();
        Self {
            access_token: value.access_token,
            refresh_token: value.refresh_token,
            expires_at,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktAccount {
    pub username: String,
    pub ids: TraktItemIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktSettings {
    pub user: TraktAccount,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktMovie {
    pub title: String,
    pub year: Option<i32>,
    pub ids: TraktItemIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktShow {
    pub title: String,
    pub year: Option<i32>,
    pub ids: TraktItemIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktEpisode {
    pub season: i64,
    pub number: i64,
    pub title: Option<String>,
    pub ids: TraktItemIds,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktWatchedMovie {
    pub plays: i64,
    pub last_watched_at: Option<DateTime<Utc>>,
    pub movie: TraktMovie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktWatchedEpisode {
    pub number: i64,
    pub plays: i64,
    pub last_watched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktWatchedSeason {
    pub number: i64,
    #[serde(default)]
    pub episodes: Vec<TraktWatchedEpisode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktWatchedShow {
    pub plays: i64,
    pub last_watched_at: Option<DateTime<Utc>>,
    pub show: TraktShow,
    #[serde(default)]
    pub seasons: Vec<TraktWatchedSeason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktMovieHistory {
    pub id: i64,
    pub watched_at: DateTime<Utc>,
    pub movie: TraktMovie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktEpisodeHistory {
    pub id: i64,
    pub watched_at: DateTime<Utc>,
    pub episode: TraktEpisode,
    pub show: TraktShow,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktMoviePlayback {
    pub id: i64,
    pub progress: f32,
    pub paused_at: DateTime<Utc>,
    pub movie: TraktMovie,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraktEpisodePlayback {
    pub id: i64,
    pub progress: f32,
    pub paused_at: DateTime<Utc>,
    pub episode: TraktEpisode,
    pub show: TraktShow,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceTokenPoll {
    Pending,
    SlowDown,
    Denied,
    Expired,
}

#[derive(Debug)]
pub enum DeviceTokenResponse {
    Approved(TraktToken),
    Waiting(DeviceTokenPoll),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
#[strum(serialize_all = "snake_case")]
pub enum TraktScrobbleAction {
    Start,
    Pause,
    Stop,
}

#[derive(Debug, Clone)]
pub struct TraktClient {
    http: reqwest::Client,
    base_url: String,
    client_id: String,
    client_secret: Secret<String>,
}

impl TraktClient {
    pub fn new(
        base_url: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: Secret<String>,
    ) -> Self {
        Self {
            http: reqwest::Client::new(),
            base_url: base_url
                .into()
                .trim_end_matches('/')
                .to_string(),
            client_id: client_id.into(),
            client_secret,
        }
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        self.http
            .request(method, format!("{}{path}", self.base_url))
            .header("trakt-api-version", "2")
            .header("trakt-api-key", &self.client_id)
            .header("Accept", "application/json")
            .header("User-Agent", "remux-server")
    }

    fn authenticated(
        &self,
        method: Method,
        path: &str,
        credentials: &TraktCredentials,
    ) -> reqwest::RequestBuilder {
        self.request(method, path)
            .bearer_auth(
                credentials
                    .access_token
                    .expose(),
            )
    }

    pub async fn begin_device_auth(&self) -> Result<TraktDeviceCode, TraktError> {
        self.send_json(
            self.request(Method::POST, "/oauth/device/code")
                .json(&serde_json::json!({ "client_id": self.client_id })),
        )
        .await
    }

    pub async fn poll_device_token(
        &self,
        device_code: &str,
    ) -> Result<DeviceTokenResponse, TraktError> {
        let response = self
            .request(Method::POST, "/oauth/device/token")
            .json(&serde_json::json!({
                "code": device_code,
                "client_id": self.client_id,
                "client_secret": self.client_secret.expose(),
            }))
            .send()
            .await?;
        match response
            .status()
            .as_u16()
        {
            200 => Ok(DeviceTokenResponse::Approved(
                response
                    .json()
                    .await?,
            )),
            409 => Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::Pending)),
            410 => Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::Expired)),
            418 => Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::Denied)),
            429 => Ok(DeviceTokenResponse::Waiting(DeviceTokenPoll::SlowDown)),
            _ => Err(Self::response_error(response).await),
        }
    }

    pub async fn refresh(
        &self,
        refresh_token: &str,
    ) -> Result<TraktCredentials, TraktError> {
        let token: TraktToken = self
            .send_json(
                self.request(Method::POST, "/oauth/token")
                    .json(&serde_json::json!({
                        "refresh_token": refresh_token,
                        "client_id": self.client_id,
                        "client_secret": self.client_secret.expose(),
                        "redirect_uri": "urn:ietf:wg:oauth:2.0:oob",
                        "grant_type": "refresh_token",
                    })),
            )
            .await?;
        Ok(token.into())
    }

    pub async fn revoke(&self, access_token: &str) -> Result<(), TraktError> {
        self.send_empty(
            self.request(Method::POST, "/oauth/revoke")
                .json(&serde_json::json!({
                    "token": access_token,
                    "client_id": self.client_id,
                    "client_secret": self.client_secret.expose(),
                })),
        )
        .await
    }

    pub async fn settings(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<TraktSettings, TraktError> {
        self.send_json(self.authenticated(Method::GET, "/users/settings", credentials))
            .await
    }

    pub async fn watched_movies(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktWatchedMovie>, TraktError> {
        self.get_all("/sync/watched/movies", credentials)
            .await
    }

    pub async fn watched_shows(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktWatchedShow>, TraktError> {
        self.get_all("/sync/watched/shows?extended=progress", credentials)
            .await
    }

    pub async fn movie_history(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktMovieHistory>, TraktError> {
        self.get_all("/sync/history/movies", credentials)
            .await
    }

    pub async fn episode_history(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktEpisodeHistory>, TraktError> {
        self.get_all("/sync/history/episodes", credentials)
            .await
    }

    pub async fn movie_playback(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktMoviePlayback>, TraktError> {
        self.get_all("/sync/playback/movies", credentials)
            .await
    }

    pub async fn episode_playback(
        &self,
        credentials: &TraktCredentials,
    ) -> Result<Vec<TraktEpisodePlayback>, TraktError> {
        self.get_all("/sync/playback/episodes", credentials)
            .await
    }

    pub async fn scrobble(
        &self,
        action: TraktScrobbleAction,
        payload: &serde_json::Value,
        credentials: &TraktCredentials,
    ) -> Result<(), TraktError> {
        self.send_empty(
            self.authenticated(
                Method::POST,
                &format!("/scrobble/{action}"),
                credentials,
            )
            .json(payload),
        )
        .await
    }

    pub async fn history(
        &self,
        remove: bool,
        payload: &serde_json::Value,
        credentials: &TraktCredentials,
    ) -> Result<(), TraktError> {
        let path = if remove {
            "/sync/history/remove"
        } else {
            "/sync/history"
        };
        self.send_empty(
            self.authenticated(Method::POST, path, credentials)
                .json(payload),
        )
        .await
    }

    async fn get_all<T: DeserializeOwned>(
        &self,
        path: &str,
        credentials: &TraktCredentials,
    ) -> Result<Vec<T>, TraktError> {
        let mut page = 1u32;
        let mut result = Vec::new();
        loop {
            let separator = if path.contains('?') { '&' } else { '?' };
            let request = self.authenticated(
                Method::GET,
                &format!("{path}{separator}page={page}&limit=250"),
                credentials,
            );
            let response = request
                .send()
                .await?;
            if !response
                .status()
                .is_success()
            {
                return Err(Self::response_error(response).await);
            }
            let page_count = response
                .headers()
                .get("x-pagination-page-count")
                .and_then(|v| {
                    v.to_str()
                        .ok()
                })
                .and_then(|v| {
                    v.parse::<u32>()
                        .ok()
                });
            let items: Vec<T> = response
                .json()
                .await?;
            let count = items.len();
            result.extend(items);
            if count == 0
                || page_count.is_none()
                || page >= page_count.unwrap_or(page)
                || page >= 500
            {
                break;
            }
            page += 1;
        }
        Ok(result)
    }

    async fn send_json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, TraktError> {
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
    ) -> Result<(), TraktError> {
        let response = request
            .send()
            .await?;
        if response
            .status()
            .is_success()
            || response.status() == StatusCode::CONFLICT
        {
            return Ok(());
        }
        Err(Self::response_error(response).await)
    }

    async fn response_error(response: Response) -> TraktError {
        let status = response.status();
        if status == StatusCode::UNAUTHORIZED {
            return TraktError::Unauthorized;
        }
        if status == StatusCode::TOO_MANY_REQUESTS {
            let retry_after = response
                .headers()
                .get("retry-after")
                .and_then(|v| {
                    v.to_str()
                        .ok()
                })
                .and_then(|v| {
                    v.parse::<u64>()
                        .ok()
                })
                .map(Duration::from_secs);
            return TraktError::RateLimited { retry_after };
        }
        let message = response
            .text()
            .await
            .unwrap_or_else(|_| "request failed".to_string());
        TraktError::Http {
            status: status.as_u16(),
            message,
        }
    }
}
