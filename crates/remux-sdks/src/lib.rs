#![allow(warnings)]

pub mod aio;
pub mod remux;
pub mod tmdb;

use http::{HeaderMap, HeaderValue, Method, header};
use itertools::Itertools;
use md5;
use remux_utils::Store;
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::fmt;
use std::iter;
use std::ops;
use std::sync::Arc;
use std::time::Duration;

static HTTP_CACHE: std::sync::LazyLock<Store> =
    std::sync::LazyLock::new(|| Store::new(50));

static SHARED_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(reqwest::Client::new);

fn hash_key(key: &str) -> String {
    let result = md5::compute(key.as_bytes());
    format!("{:x}", result)
}

pub trait Auth: Send + Sync + Clone {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder;
}

#[derive(Clone, Debug)]
pub struct NoAuth;

impl Auth for NoAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req
    }
}

#[derive(Clone, Debug)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl Auth for BasicAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.basic_auth(&self.username, Some(&self.password))
    }
}

#[derive(Clone, Debug)]
pub struct BearerAuth {
    pub token: String,
}

impl Auth for BearerAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.bearer_auth(&self.token)
    }
}

#[derive(Clone, Debug)]
pub struct JellyfinApiKeyAuth {
    pub api_key: String,
}

impl Auth for JellyfinApiKeyAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        req.header("X-Emby-Token", &self.api_key)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("http error (status={status}) endpoint={endpoint:?}: {message}")]
    Http {
        status: u16,
        message: String,
        endpoint: Option<String>,
        body: Option<String>,
    },
    #[error("json error (status={status}) endpoint={endpoint:?}: {source}")]
    Json {
        status: u16,
        source: serde_json::Error,
        endpoint: Option<String>,
        body: Option<String>,
    },
    #[error(transparent)]
    Transport(#[from] reqwest::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    UrlEncoded(#[from] serde_urlencoded::ser::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl ClientError {
    /// Human-readable message suitable for display in a UI.
    /// For `Http` errors this is just the message field, omitting the status/endpoint noise.
    pub fn user_message(&self) -> String {
        match self {
            ClientError::Http { message, .. } => message.clone(),
            other => other.to_string(),
        }
    }
}

fn try_extract_error_message(body: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(body).ok()?;
    let title = v.get("title")?.as_str()?;
    let detail = v.get("detail").and_then(|d| d.as_str());
    Some(match detail {
        Some(d) if !d.is_empty() => format!("{title}: {d}"),
        _ => title.to_string(),
    })
}

fn default_error_mapper(status: u16, endpoint: &str, body: &str) -> ClientError {
    if status == 401 {
        ClientError::Unauthorized
    } else {
        let message = try_extract_error_message(body)
            .unwrap_or_else(|| "http error".to_string());
        ClientError::Http {
            status,
            message,
            endpoint: Some(endpoint.to_string()),
            body: Some(body.to_string()),
        }
    }
}

pub enum Body {
    Empty,
    Json(serde_json::Value),
    Form(Vec<(String, String)>),
    Text(String),
    Bytes(Vec<u8>),
}

impl Default for Body {
    fn default() -> Self {
        Body::Empty
    }
}

pub trait Endpoint {
    type Output: DeserializeOwned + Clone + Serialize;
    fn method(&self) -> Method {
        Method::GET
    }
    fn path(&self) -> String;
    fn query(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn headers(&self) -> HeaderMap {
        HeaderMap::new()
    }
    fn body(&self) -> Body {
        Body::Empty
    }
    fn cache_ttl(&self) -> Option<Duration> {
        None
    }
}

#[derive(Clone)]
pub struct RestClient<A: Auth = NoAuth> {
    http: reqwest::Client,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ClientError,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        Ok(Self {
            http: SHARED_HTTP_CLIENT.clone(),
            base: url::Url::parse(format!("{}/", base.trim_end_matches('/')).as_str())?,
            auth: Arc::new(NoAuth),
            map_error: default_error_mapper,
        })
    }
}

impl<A: Auth + Clone> RestClient<A> {
    pub fn with_auth<B: Auth + Clone>(self, auth: B) -> RestClient<B> {
        RestClient {
            http: self.http,
            base: self.base,
            auth: Arc::new(auth),
            map_error: self.map_error,
        }
    }

    pub fn with_error_mapper(mut self, f: fn(u16, &str, &str) -> ClientError) -> Self {
        self.map_error = f;
        self
    }

    pub async fn execute<EP: Endpoint + Clone>(
        &self,
        endpoint: EP,
    ) -> Result<EP::Output, ClientError> {
        let path = endpoint.path();
        let mut url = self.base.join(path.trim_matches('/')).unwrap();
        let query = endpoint.query();
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        let cache_key = hash_key(&url.to_string());

        if endpoint.cache_ttl().is_some() {
            if let Some(body) = HTTP_CACHE.get::<String>(&cache_key) {
                return Ok(serde_json::from_str(&body).map_err(|e| ClientError::Json {
                    status: 0,
                    source: e,
                    endpoint: Some(url.to_string()),
                    body: None,
                })?);
            }
        }

        let mut req = self
            .http
            .request(endpoint.method(), url.clone())
            .headers(endpoint.headers());
        req = self.auth.apply(req);
        req = match endpoint.body() {
            Body::Empty => req,
            Body::Json(v) => {
                let bytes = serde_json::to_vec(&v).map_err(|e| ClientError::Json {
                    status: 0,
                    source: e,
                    endpoint: Some(url.to_string()),
                    body: Some(v.to_string()),
                })?;
                req.header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/json"),
                )
                .body(bytes)
            }
            Body::Form(v) => {
                let encoded = serde_urlencoded::to_string(&v)?;
                req.header(
                    header::CONTENT_TYPE,
                    HeaderValue::from_static("application/x-www-form-urlencoded"),
                )
                .body(encoded)
            }
            Body::Text(s) => req.body(s),
            Body::Bytes(b) => req.body(b),
        };
        let resp = req.send().await?;
        let status = resp.status().as_u16();
        let text = resp.text().await.unwrap_or_default();
        match status {
            401 => Err(ClientError::Unauthorized),
            s if (200..300).contains(&s) => {
                // 204 No Content and similar empty responses: treat as JSON null so
                // endpoints with `type Output = ()` deserialize successfully.
                let parse_body = if text.is_empty() { "null" } else { &text };
                let result: Result<EP::Output, ClientError> =
                    serde_json::from_str::<EP::Output>(parse_body).map_err(|e| {
                        ClientError::Json {
                            status: s,
                            source: e,
                            endpoint: Some(url.to_string()),
                            body: Some(text.clone()),
                        }
                    });
                if result.is_ok() {
                    if let Some(ttl) = endpoint.cache_ttl() {
                        HTTP_CACHE.save(cache_key, text.clone(), ttl);
                    }
                }
                result
            }
            s => Err((self.map_error)(s, &url.to_string(), &text)),
        }
    }
}

pub trait CachedEndpoint: Endpoint + Sized {
    fn with_cache(self, ttl: Duration) -> Cached<Self> {
        Cached {
            endpoint: self,
            ttl,
        }
    }
}

impl<EP: Endpoint + Sized> CachedEndpoint for EP {}

#[derive(Clone)]
pub struct Cached<EP: Endpoint> {
    endpoint: EP,
    ttl: Duration,
}

impl<EP: Endpoint> Endpoint for Cached<EP> {
    type Output = EP::Output;

    fn method(&self) -> Method {
        self.endpoint.method()
    }

    fn path(&self) -> String {
        self.endpoint.path()
    }

    fn query(&self) -> Vec<(String, String)> {
        self.endpoint.query()
    }

    fn headers(&self) -> HeaderMap {
        self.endpoint.headers()
    }

    fn body(&self) -> Body {
        self.endpoint.body()
    }

    fn cache_ttl(&self) -> Option<Duration> {
        Some(self.ttl)
    }
}

#[derive(Debug, Clone, Default)]
pub struct CommaSeparatedList<T> {
    data: Vec<T>,
}

impl<T> CommaSeparatedList<T> {
    pub fn new() -> Self {
        Self { data: Vec::new() }
    }
}

impl<T> From<Vec<T>> for CommaSeparatedList<T> {
    fn from(data: Vec<T>) -> Self {
        Self { data }
    }
}

impl<T> iter::FromIterator<T> for CommaSeparatedList<T> {
    fn from_iter<I>(iter: I) -> Self
    where
        I: IntoIterator<Item = T>,
    {
        Self {
            data: iter.into_iter().collect(),
        }
    }
}

impl<T> ops::Deref for CommaSeparatedList<T> {
    type Target = Vec<T>;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl<T> ops::DerefMut for CommaSeparatedList<T> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.data
    }
}

impl<T> fmt::Display for CommaSeparatedList<T>
where
    T: fmt::Display,
{
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}", self.data.iter().format(","))
    }
}

pub fn deserialize_option_number_from_string<'de, D>(
    deserializer: D,
) -> Result<Option<f64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(f64),
    }

    let value = Option::<StringOrNumber>::deserialize(deserializer)?;
    match value {
        Some(StringOrNumber::String(s)) => {
            if s.trim().is_empty() || s.to_lowercase() == "n/a" {
                Ok(None)
            } else {
                s.parse::<f64>().map(Some).map_err(serde::de::Error::custom)
            }
        }
        Some(StringOrNumber::Number(n)) => Ok(Some(n)),
        None => Ok(None),
    }
}

/// Deserializes an optional `NaiveDate` from a string, treating empty strings as `None`.
/// TMDB returns `""` instead of `null` for missing dates, which chrono refuses to parse.
pub fn deserialize_option_naive_date<'de, D>(
    deserializer: D,
) -> Result<Option<chrono::NaiveDate>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(deserializer)?;
    match s {
        None => Ok(None),
        Some(ref v) if v.is_empty() => Ok(None),
        Some(s) => s
            .parse::<chrono::NaiveDate>()
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

impl From<aio::MediaType> for remux::MediaType {
    fn from(kind: aio::MediaType) -> Self {
        match kind {
            aio::MediaType::Movie => remux::MediaType::Movie,
            aio::MediaType::Series => remux::MediaType::Series,
            _ => remux::MediaType::Unknown,
        }
    }
}

impl From<remux::MediaType> for aio::MediaType {
    fn from(kind: remux::MediaType) -> Self {
        match kind {
            remux::MediaType::Movie => aio::MediaType::Movie,
            remux::MediaType::Series => aio::MediaType::Series,
            remux::MediaType::Episode => aio::MediaType::Series,
            _ => aio::MediaType::Movie,
        }
    }
}
