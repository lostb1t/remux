#![allow(warnings)]

pub mod deezer;
pub mod introdb;
pub mod kitsu;
pub mod remux;
pub mod remuxdb;
pub mod stremio;
pub mod tmdb;
pub mod trakt;

use http::{HeaderMap, HeaderValue, Method, header};
use itertools::Itertools;
use md5;
use remux_utils::Store;
use reqwest_middleware::ClientBuilder as MwClientBuilder;
pub use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::{RetryPolicy, RetryTransientMiddleware};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, fmt, iter, ops, sync::Arc, time::Duration};

static HTTP_CACHE: std::sync::LazyLock<Store> =
    std::sync::LazyLock::new(|| Store::new_weighted(32 * 1024 * 1024)); // 32 MB weight cap

static SHARED_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(reqwest::Client::new);

pub fn clear_http_cache() {
    HTTP_CACHE.clear();
}

/// Returns `(entry_count, weighted_size)` for the HTTP response cache.
pub fn http_cache_stats() -> (u64, u64) {
    (HTTP_CACHE.entry_count(), HTTP_CACHE.weighted_size())
}

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
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
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
    let title = v
        .get("title")?
        .as_str()?;
    let detail = v
        .get("detail")
        .and_then(|d| d.as_str());
    Some(match detail {
        Some(d) if !d.is_empty() => format!("{title}: {d}"),
        _ => title.to_string(),
    })
}

fn default_error_mapper(status: u16, endpoint: &str, body: &str) -> ClientError {
    if status == 401 {
        ClientError::Unauthorized
    } else {
        let message =
            try_extract_error_message(body).unwrap_or_else(|| "http error".to_string());
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
    type Output: DeserializeOwned + Clone + Serialize + Send + Sync + 'static;

    fn path(&self) -> String;

    fn query_params(&self) -> impl serde::Serialize + '_ {
        ()
    }

    fn query(&self) -> Vec<(String, String)> {
        serde_urlencoded::to_string(&self.query_params())
            .unwrap_or_default()
            .split('&')
            .filter(|s| !s.is_empty())
            .filter_map(|pair| {
                let (k, v) = pair.split_once('=')?;
                Some((k.to_string(), v.to_string()))
            })
            .collect()
    }

    fn method(&self) -> Method {
        Method::GET
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

    fn retry_policy(&self) -> Option<Arc<dyn RetryPolicy + Send + Sync>> {
        None
    }
}

#[derive(Clone)]
pub struct RestClient<A: Auth = NoAuth> {
    http: reqwest::Client,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ClientError,
    default_retry: Option<Arc<dyn RetryPolicy + Send + Sync>>,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        Ok(Self {
            http: SHARED_HTTP_CLIENT.clone(),
            base: url::Url::parse(format!("{}/", base.trim_end_matches('/')).as_str())?,
            auth: Arc::new(NoAuth),
            map_error: default_error_mapper,
            default_retry: None,
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
            default_retry: self.default_retry,
        }
    }

    pub fn with_error_mapper(mut self, f: fn(u16, &str, &str) -> ClientError) -> Self {
        self.map_error = f;
        self
    }

    pub fn with_retry<P: RetryPolicy + Send + Sync + 'static>(
        mut self,
        policy: P,
    ) -> Self {
        self.default_retry = Some(Arc::new(policy));
        self
    }

    /// Owned result. Prefer `execute_arc` when the value is only read — on a cache
    /// hit this has to deep-copy the payload to hand back a `T`.
    pub async fn execute<EP: Endpoint + Clone>(
        &self,
        endpoint: EP,
    ) -> Result<EP::Output, ClientError> {
        self.execute_arc(endpoint)
            .await
            .map(|arc| {
                // Uncached responses are uniquely owned here, so this unwraps
                // without copying; only cache hits fall back to a clone.
                Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone())
            })
    }

    /// Shared result — no deep copy on a cache hit.
    pub async fn execute_arc<EP: Endpoint + Clone>(
        &self,
        endpoint: EP,
    ) -> Result<Arc<EP::Output>, ClientError> {
        let path = endpoint.path();
        let mut url = self
            .base
            .join(path.trim_matches('/'))
            .unwrap();
        // query() returns already-percent-encoded key=value pairs from serde_urlencoded.
        // Reassemble them into a raw query string and set it directly — feeding them
        // into query_pairs_mut().extend_pairs() would double-encode the values
        // (e.g. comma → %2C → %252C), breaking TMDB's append_to_response parameter.
        let query = endpoint.query();
        if !query.is_empty() {
            let qs: String = query
                .iter()
                .map(|(k, v)| format!("{k}={v}"))
                .collect::<Vec<_>>()
                .join("&");
            url.set_query(Some(&qs));
        }
        let cache_key = hash_key(&url.to_string());

        if endpoint
            .cache_ttl()
            .is_some()
        {
            if let Some(value) = HTTP_CACHE.get::<EP::Output>(&cache_key) {
                return Ok(value);
            }
        }

        let mut req = self
            .http
            .request(endpoint.method(), url.clone())
            .headers(endpoint.headers());
        req = self
            .auth
            .apply(req);
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
        let retry = endpoint
            .retry_policy()
            .or_else(|| {
                self.default_retry
                    .clone()
            });
        let resp = if let Some(policy) = retry {
            let request = req
                .build()
                .map_err(ClientError::Transport)?;
            let mw_client = MwClientBuilder::new(
                self.http
                    .clone(),
            )
            .with(RetryTransientMiddleware::new_with_policy(DynRetryPolicy(
                policy,
            )))
            .build();
            mw_client
                .execute(request)
                .await
                .map_err(|e| match e {
                    reqwest_middleware::Error::Reqwest(e) => ClientError::Transport(e),
                    reqwest_middleware::Error::Middleware(e) => ClientError::Other(e),
                })?
        } else {
            req.send()
                .await?
        };
        let status = resp
            .status()
            .as_u16();
        if status == 429 {
            let retry_after_secs = resp
                .headers()
                .get("Retry-After")
                .and_then(|v| {
                    v.to_str()
                        .ok()
                })
                .and_then(|s| {
                    s.parse::<u64>()
                        .ok()
                })
                .unwrap_or(60);
            return Err(ClientError::RateLimited { retry_after_secs });
        }
        let text = resp
            .text()
            .await
            .unwrap_or_default();
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
                let arc = result.map(Arc::new)?;
                if let Some(ttl) = endpoint.cache_ttl() {
                    let weight = text
                        .len()
                        .min(u32::MAX as usize) as u32;
                    HTTP_CACHE.save_arc_with_weight(
                        cache_key,
                        Arc::clone(&arc),
                        weight,
                        ttl,
                    );
                }
                Ok(arc)
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
        self.endpoint
            .method()
    }

    fn path(&self) -> String {
        self.endpoint
            .path()
    }

    fn query(&self) -> Vec<(String, String)> {
        self.endpoint
            .query()
    }

    fn headers(&self) -> HeaderMap {
        self.endpoint
            .headers()
    }

    fn body(&self) -> Body {
        self.endpoint
            .body()
    }

    fn cache_ttl(&self) -> Option<Duration> {
        Some(self.ttl)
    }
}

// Wraps Arc<dyn RetryPolicy> so it can be passed to RetryTransientMiddleware,
// which requires a concrete type implementing RetryPolicy + Send + Sync.
struct DynRetryPolicy(Arc<dyn RetryPolicy + Send + Sync>);

impl RetryPolicy for DynRetryPolicy {
    fn should_retry(
        &self,
        request_start_time: std::time::SystemTime,
        n_past_retries: u32,
    ) -> reqwest_retry::RetryDecision {
        self.0
            .should_retry(request_start_time, n_past_retries)
    }
}

#[derive(Clone)]
pub struct WithRetry<EP: Endpoint> {
    endpoint: EP,
    policy: Arc<dyn RetryPolicy + Send + Sync>,
}

impl<EP: Endpoint + Clone> Endpoint for WithRetry<EP> {
    type Output = EP::Output;

    fn method(&self) -> Method {
        self.endpoint
            .method()
    }

    fn path(&self) -> String {
        self.endpoint
            .path()
    }

    fn query(&self) -> Vec<(String, String)> {
        self.endpoint
            .query()
    }

    fn headers(&self) -> HeaderMap {
        self.endpoint
            .headers()
    }

    fn body(&self) -> Body {
        self.endpoint
            .body()
    }

    fn cache_ttl(&self) -> Option<Duration> {
        self.endpoint
            .cache_ttl()
    }

    fn retry_policy(&self) -> Option<Arc<dyn RetryPolicy + Send + Sync>> {
        Some(
            self.policy
                .clone(),
        )
    }
}

pub trait RetryableEndpoint: Endpoint + Sized {
    fn with_retry<P: RetryPolicy + Send + Sync + 'static>(
        self,
        policy: P,
    ) -> WithRetry<Self> {
        WithRetry {
            endpoint: self,
            policy: Arc::new(policy),
        }
    }
}

impl<EP: Endpoint + Sized> RetryableEndpoint for EP {}

/// Wraps an endpoint and appends extra query parameters to every request.
/// Used by `StremioService` to forward manifest-URL query params to all resource calls.
#[derive(Clone)]
pub struct WithExtraQuery<EP: Endpoint> {
    pub endpoint: EP,
    pub extra: Vec<(String, String)>,
}

impl<EP: Endpoint> Endpoint for WithExtraQuery<EP> {
    type Output = EP::Output;

    fn path(&self) -> String {
        self.endpoint
            .path()
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut q = self
            .endpoint
            .query();
        q.extend(
            self.extra
                .iter()
                .cloned(),
        );
        q
    }

    fn method(&self) -> Method {
        self.endpoint
            .method()
    }

    fn headers(&self) -> HeaderMap {
        self.endpoint
            .headers()
    }

    fn body(&self) -> Body {
        self.endpoint
            .body()
    }

    fn cache_ttl(&self) -> Option<Duration> {
        self.endpoint
            .cache_ttl()
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
            data: iter
                .into_iter()
                .collect(),
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
        write!(
            f,
            "{}",
            self.data
                .iter()
                .format(",")
        )
    }
}

impl<'de, T> Deserialize<'de> for CommaSeparatedList<T>
where
    T: std::str::FromStr,
{
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Visitor;
        use std::marker::PhantomData;

        struct CslVisitor<T>(PhantomData<T>);

        impl<'de, T: std::str::FromStr> Visitor<'de> for CslVisitor<T> {
            type Value = CommaSeparatedList<T>;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a comma-separated string or sequence of strings")
            }

            fn visit_none<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CommaSeparatedList::new())
            }

            fn visit_unit<E: serde::de::Error>(self) -> Result<Self::Value, E> {
                Ok(CommaSeparatedList::new())
            }

            fn visit_str<E: serde::de::Error>(self, v: &str) -> Result<Self::Value, E> {
                Ok(CommaSeparatedList {
                    data: v
                        .split(',')
                        .filter(|s| !s.is_empty())
                        .filter_map(|s| {
                            s.trim()
                                .parse::<T>()
                                .ok()
                        })
                        .collect(),
                })
            }

            fn visit_seq<A: serde::de::SeqAccess<'de>>(
                self,
                mut seq: A,
            ) -> Result<Self::Value, A::Error> {
                let mut data = Vec::new();
                while let Some(val) = seq.next_element::<String>()? {
                    data.extend(
                        val.split(',')
                            .filter(|s| !s.is_empty())
                            .filter_map(|s| {
                                s.trim()
                                    .parse::<T>()
                                    .ok()
                            }),
                    );
                }
                Ok(CommaSeparatedList { data })
            }
        }

        d.deserialize_any(CslVisitor(PhantomData))
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
            if s.trim()
                .is_empty()
                || s.to_lowercase() == "n/a"
            {
                Ok(None)
            } else {
                s.parse::<f64>()
                    .map(Some)
                    .map_err(serde::de::Error::custom)
            }
        }
        Some(StringOrNumber::Number(n)) => Ok(Some(n)),
        None => Ok(None),
    }
}

pub fn deserialize_option_i64_from_string<'de, D>(
    deserializer: D,
) -> Result<Option<i64>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumber {
        String(String),
        Number(i64),
    }

    let value = Option::<StringOrNumber>::deserialize(deserializer)?;
    match value {
        Some(StringOrNumber::String(s)) => s
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(serde::de::Error::custom),
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

impl From<stremio::MediaType> for remux::MediaType {
    fn from(kind: stremio::MediaType) -> Self {
        match kind {
            stremio::MediaType::Movie => remux::MediaType::Movie,
            stremio::MediaType::Series => remux::MediaType::Series,
            _ => remux::MediaType::Unknown,
        }
    }
}

impl From<remux::MediaType> for stremio::MediaType {
    fn from(kind: remux::MediaType) -> Self {
        match kind {
            remux::MediaType::Movie => stremio::MediaType::Movie,
            remux::MediaType::Series => stremio::MediaType::Series,
            remux::MediaType::Episode => stremio::MediaType::Series,
            _ => stremio::MediaType::Movie,
        }
    }
}
