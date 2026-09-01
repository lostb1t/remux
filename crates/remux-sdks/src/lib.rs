#![allow(warnings)]

pub mod deezer;
pub mod introdb;
pub mod kitsu;
pub mod remux;
pub mod remuxdb;
pub mod stremio;
pub mod tmdb;
pub mod trakt;

use bytes::Bytes;
use http::{Extensions, HeaderMap, HeaderValue, Method, header};
use itertools::Itertools;
use remux_utils::Secret;
use reqwest_middleware::{ClientBuilder as MwClientBuilder, ClientWithMiddleware};
pub use reqwest_retry::policies::ExponentialBackoff;
use reqwest_retry::{RetryPolicy, RetryTransientMiddleware};
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use std::{collections::HashMap, fmt, iter, ops, sync::Arc, time::Duration};
#[cfg(not(target_arch = "wasm32"))]
use {
    async_trait::async_trait,
    md5,
    remux_utils::Store,
    reqwest_middleware::{Middleware, Next},
};

#[cfg(not(target_arch = "wasm32"))]
static HTTP_CACHE: std::sync::LazyLock<Store> =
    std::sync::LazyLock::new(|| Store::new_weighted(32 * 1024 * 1024)); // 32 MB weight cap

static SHARED_HTTP_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(reqwest::Client::new);

#[cfg(not(target_arch = "wasm32"))]
pub fn clear_http_cache() {
    HTTP_CACHE.clear();
}

/// Returns `(entry_count, weighted_size)` for the HTTP response cache.
#[cfg(not(target_arch = "wasm32"))]
pub fn http_cache_stats() -> (u64, u64) {
    (HTTP_CACHE.entry_count(), HTTP_CACHE.weighted_size())
}

#[cfg(not(target_arch = "wasm32"))]
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
        let val = format!(
            r#"MediaBrowser Client="remux", Device="server", DeviceId="remux-server", Version="1.0.0", Token="{}""#,
            self.api_key
        );
        match HeaderValue::from_str(&val) {
            Ok(v) => req.header(header::AUTHORIZATION, v),
            Err(_) => req.header("X-Emby-Token", &self.api_key),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("unauthorized")]
    Unauthorized,
    #[error("rate limited, retry after {retry_after_secs}s")]
    RateLimited { retry_after_secs: u64 },
    #[error("http error (status={status}): {message}")]
    Http {
        status: u16,
        message: String,
        endpoint: Option<Secret<String>>,
        body: Option<Secret<String>>,
    },
    #[error("json error (status={status}): {source}")]
    Json {
        status: u16,
        source: serde_json::Error,
        endpoint: Option<Secret<String>>,
        body: Option<Secret<String>>,
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

#[cfg(test)]
mod client_error_tests {
    use super::*;

    #[test]
    fn error_formatting_redacts_credentialed_endpoints() {
        let secret = "do-not-log-this-token";
        let error = ClientError::Http {
            status: 502,
            message: "upstream unavailable".to_string(),
            endpoint: Some(Secret::new(format!(
                "https://addon.example/manifest.json?token={secret}"
            ))),
            body: None,
        };
        assert!(
            !error
                .to_string()
                .contains(secret)
        );
        assert!(!format!("{error:?}").contains(secret));
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
            endpoint: Some(Secret::new(endpoint.to_string())),
            body: Some(Secret::new(body.to_string())),
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

/// Per-request cache configuration.
///
/// Build with [`CacheOptions::new`]. Implements `From<Duration>` so existing
/// `.with_cache(Duration::from_secs(60))` call sites keep compiling.
#[derive(Clone)]
pub struct CacheOptions {
    pub ttl: Duration,
    /// Status codes to cache. Defaults to `&[200]`. 2xx statuses are cached
    /// and deserialized normally; non-2xx statuses are cached and returned as
    /// `None` (the endpoint `Output` must be `Option<T>`).
    pub on_statuses: &'static [u16],
}

impl CacheOptions {
    pub fn new(ttl: Duration) -> Self {
        Self {
            ttl,
            on_statuses: &[200],
        }
    }

    pub fn on_statuses(mut self, statuses: &'static [u16]) -> Self {
        self.on_statuses = statuses;
        self
    }
}

impl From<Duration> for CacheOptions {
    fn from(ttl: Duration) -> Self {
        Self::new(ttl)
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

    fn cache_options(&self) -> Option<CacheOptions> {
        None
    }

    /// Whether and how long to cache *this* response. `None` skips caching.
    /// Defaults to `cache_options().ttl`. Override in [`Cached`] for one-off logic.
    fn should_cache(&self, _response: &Self::Output) -> Option<Duration> {
        self.cache_options()
            .map(|o| o.ttl)
    }
}

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone, Copy)]
struct CacheTTL(Duration);

#[cfg(not(target_arch = "wasm32"))]
#[derive(Clone)]
struct CachedResponse {
    status: u16,
    body: String,
}

#[cfg(not(target_arch = "wasm32"))]
struct InMemoryCacheMiddleware;

#[cfg(not(target_arch = "wasm32"))]
#[async_trait]
impl Middleware for InMemoryCacheMiddleware {
    async fn handle(
        &self,
        req: reqwest::Request,
        extensions: &mut Extensions,
        next: Next<'_>,
    ) -> reqwest_middleware::Result<reqwest::Response> {
        let ttl = extensions
            .get::<CacheTTL>()
            .copied();
        // Derive the cache key from the pre-redirect URL so hits are consistent
        // regardless of whether the server redirects the request.
        let key = ttl.map(|_| {
            hash_key(
                req.url()
                    .as_str(),
            )
        });

        if let Some(ref k) = key {
            if let Some(cached) = HTTP_CACHE.get::<CachedResponse>(k) {
                let resp = http::Response::builder()
                    .status(cached.status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Bytes::from(
                        cached
                            .body
                            .clone(),
                    ))
                    .unwrap();
                return Ok(reqwest::Response::from(resp));
            }
        }

        let resp = next
            .run(req, extensions)
            .await?;

        if let (Some(CacheTTL(ttl)), Some(k)) = (ttl, key) {
            if resp
                .status()
                .is_success()
            {
                let status = resp.status();
                let text = resp
                    .text()
                    .await
                    .map_err(reqwest_middleware::Error::Reqwest)?;
                let weight = text
                    .len()
                    .min(u32::MAX as usize) as u32;
                HTTP_CACHE.save_arc_with_weight(
                    k,
                    Arc::new(CachedResponse {
                        status: status.as_u16(),
                        body: text.clone(),
                    }),
                    weight,
                    ttl,
                );
                let rebuilt = http::Response::builder()
                    .status(status)
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Bytes::from(text))
                    .unwrap();
                return Ok(reqwest::Response::from(rebuilt));
            }
        }

        Ok(resp)
    }
}

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

fn build_mw(retry: Option<Arc<dyn RetryPolicy + Send + Sync>>) -> ClientWithMiddleware {
    #[cfg(not(target_arch = "wasm32"))]
    let builder =
        MwClientBuilder::new(SHARED_HTTP_CLIENT.clone()).with(InMemoryCacheMiddleware);
    #[cfg(target_arch = "wasm32")]
    let builder = MwClientBuilder::new(SHARED_HTTP_CLIENT.clone());
    match retry {
        Some(policy) => builder
            .with(
                RetryTransientMiddleware::new_with_policy(DynRetryPolicy(policy))
                    .with_retry_log_level(tracing::Level::DEBUG),
            )
            .build(),
        None => builder.build(),
    }
}

#[derive(Clone)]
pub struct RestClient<A: Auth = NoAuth> {
    mw: ClientWithMiddleware,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ClientError,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        Ok(Self {
            mw: build_mw(None),
            base: url::Url::parse(format!("{}/", base.trim_end_matches('/')).as_str())?,
            auth: Arc::new(NoAuth),
            map_error: default_error_mapper,
        })
    }
}

impl<A: Auth + Clone> RestClient<A> {
    pub fn with_auth<B: Auth + Clone>(self, auth: B) -> RestClient<B> {
        RestClient {
            mw: self.mw,
            base: self.base,
            auth: Arc::new(auth),
            map_error: self.map_error,
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
        self.mw = build_mw(Some(Arc::new(policy)));
        self
    }

    pub async fn execute<EP: Endpoint + Clone>(
        &self,
        endpoint: EP,
    ) -> Result<EP::Output, ClientError> {
        self.execute_arc(endpoint)
            .await
            .map(|arc| Arc::try_unwrap(arc).unwrap_or_else(|arc| (*arc).clone()))
    }

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

        let mut req = SHARED_HTTP_CLIENT
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
                    endpoint: Some(Secret::new(url.to_string())),
                    body: Some(Secret::new(v.to_string())),
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
        let request = req
            .build()
            .map_err(ClientError::Transport)?;

        let mut ext = Extensions::new();
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(opts) = endpoint.cache_options() {
            ext.insert(CacheTTL(opts.ttl));
        }

        let resp = self
            .mw
            .execute_with_extensions(request, &mut ext)
            .await
            .map_err(|e| match e {
                reqwest_middleware::Error::Reqwest(e) => ClientError::Transport(e),
                reqwest_middleware::Error::Middleware(e) => ClientError::Other(e),
            })?;

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
        let on_statuses = endpoint
            .cache_options()
            .map(|o| o.on_statuses)
            .unwrap_or(&[]);
        match status {
            401 => Err(ClientError::Unauthorized),
            s if on_statuses.contains(&s) && (200..300).contains(&s) => {
                // 204 No Content and similar empty responses: treat as JSON null so
                // endpoints with `type Output = ()` deserialize successfully.
                let parse_body = if text.is_empty() { "null" } else { &text };
                let arc = serde_json::from_str::<EP::Output>(parse_body)
                    .map(Arc::new)
                    .map_err(|e| ClientError::Json {
                        status: s,
                        source: e,
                        endpoint: Some(Secret::new(url.to_string())),
                        body: Some(Secret::new(text.clone())),
                    })?;
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ttl) = endpoint.should_cache(&arc) {
                    let weight = text
                        .len()
                        .min(u32::MAX as usize) as u32;
                    HTTP_CACHE.save_arc_with_weight(
                        hash_key(url.as_str()),
                        Arc::new(CachedResponse {
                            status: s,
                            body: text,
                        }),
                        weight,
                        ttl,
                    );
                }
                Ok(arc)
            }
            s if on_statuses.contains(&s) => {
                // Non-2xx in on_statuses: cache and return None (Output must be Option<T>).
                let arc = serde_json::from_str::<EP::Output>("null")
                    .map(Arc::new)
                    .map_err(|e| ClientError::Json {
                        status: s,
                        source: e,
                        endpoint: Some(Secret::new(url.to_string())),
                        body: Some(Secret::new(text.clone())),
                    })?;
                #[cfg(not(target_arch = "wasm32"))]
                if let Some(ttl) = endpoint.should_cache(&arc) {
                    HTTP_CACHE.save_arc_with_weight(
                        hash_key(url.as_str()),
                        Arc::new(CachedResponse {
                            status: s,
                            body: "null".to_string(),
                        }),
                        4,
                        ttl,
                    );
                }
                Ok(arc)
            }
            s if (200..300).contains(&s) => {
                // 2xx not in on_statuses: deserialize without caching.
                let parse_body = if text.is_empty() { "null" } else { &text };
                serde_json::from_str::<EP::Output>(parse_body)
                    .map(Arc::new)
                    .map_err(|e| ClientError::Json {
                        status: s,
                        source: e,
                        endpoint: Some(Secret::new(url.to_string())),
                        body: Some(Secret::new(text.clone())),
                    })
            }
            s => Err((self.map_error)(s, &url.to_string(), &text)),
        }
    }
}

pub trait CachedEndpoint: Endpoint + Sized {
    fn with_cache(self, opts: impl Into<CacheOptions>) -> Cached<Self> {
        Cached {
            endpoint: self,
            opts: opts.into(),
            should_cache_fn: None,
        }
    }
}

impl<EP: Endpoint + Sized> CachedEndpoint for EP {}

#[derive(Clone)]
pub struct Cached<EP: Endpoint> {
    endpoint: EP,
    opts: CacheOptions,
    should_cache_fn: Option<fn(&EP::Output) -> Option<Duration>>,
}

impl<EP: Endpoint> Cached<EP> {
    pub fn should_cache(self, f: fn(&EP::Output) -> Option<Duration>) -> Self {
        Self {
            should_cache_fn: Some(f),
            ..self
        }
    }
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

    fn cache_options(&self) -> Option<CacheOptions> {
        Some(
            self.opts
                .clone(),
        )
    }

    fn should_cache(&self, response: &Self::Output) -> Option<Duration> {
        match self.should_cache_fn {
            Some(f) => f(response),
            None => Some(
                self.opts
                    .ttl,
            ),
        }
    }
}

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

    fn cache_options(&self) -> Option<CacheOptions> {
        self.endpoint
            .cache_options()
    }

    fn should_cache(&self, response: &Self::Output) -> Option<Duration> {
        self.endpoint
            .should_cache(response)
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

pub fn deserialize_option_string_from_any<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum StringOrNumberOrBool {
        String(String),
        Number(serde_json::Number),
        Bool(bool),
    }

    let value = Option::<StringOrNumberOrBool>::deserialize(deserializer)?;
    Ok(match value {
        Some(StringOrNumberOrBool::String(s)) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        Some(StringOrNumberOrBool::Number(n)) => Some(n.to_string()),
        Some(StringOrNumberOrBool::Bool(b)) => Some(b.to_string()),
        None => None,
    })
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
            _ => remux::MediaType::Other,
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

#[cfg(test)]
mod cache_tests {
    use super::*;

    /// `Vec<String>` so "the response carries no answer" is just `is_empty`,
    /// standing in for a `/find` with no results.
    #[derive(Clone)]
    struct Probe {
        path: String,
    }

    impl Endpoint for Probe {
        type Output = Vec<String>;

        fn path(&self) -> String {
            self.path
                .clone()
        }
    }

    /// Long enough that nothing here reaches it.
    const NEVER: Duration = Duration::from_secs(600);
    const BRIEF: Duration = Duration::from_millis(300);

    fn is_empty(response: &Vec<String>) -> bool {
        response.is_empty()
    }

    /// `HTTP_CACHE` is process-wide and keyed on the url, and httpmock pools
    /// its servers, so each test needs its own path.
    fn probe<'s>(
        server: &'s httpmock::MockServer,
        path: &str,
        body: serde_json::Value,
    ) -> (httpmock::Mock<'s>, RestClient<NoAuth>, Probe) {
        let mock = server.mock(|when, then| {
            when.path(format!("/{path}"));
            then.status(200)
                .json_body(body);
        });
        (
            mock,
            RestClient::new(&server.base_url()).unwrap(),
            Probe {
                path: path.to_string(),
            },
        )
    }

    async fn elapse() {
        tokio::time::sleep(BRIEF * 2).await;
    }

    #[tokio::test]
    async fn a_response_matching_the_rule_is_re_fetched_after_the_short_ttl() {
        let server = httpmock::MockServer::start();
        let (mock, client, probe) = probe(&server, "matching", serde_json::json!([]));
        let endpoint = probe
            .with_cache(NEVER)
            .should_cache(|r| Some(if is_empty(r) { BRIEF } else { NEVER }));

        client
            .execute(endpoint.clone())
            .await
            .unwrap();
        client
            .execute(endpoint.clone())
            .await
            .unwrap();
        assert_eq!(mock.hits(), 1, "served from cache while it lived");

        elapse().await;
        client
            .execute(endpoint)
            .await
            .unwrap();
        assert_eq!(mock.hits(), 2, "asked again once the short TTL was up");
    }

    #[tokio::test]
    async fn a_response_the_rule_rejects_keeps_the_full_ttl() {
        let server = httpmock::MockServer::start();
        let (mock, client, probe) =
            probe(&server, "rejected", serde_json::json!(["found"]));
        let endpoint = probe
            .with_cache(NEVER)
            .should_cache(|r| Some(if is_empty(r) { BRIEF } else { NEVER }));

        client
            .execute(endpoint.clone())
            .await
            .unwrap();
        elapse().await;
        client
            .execute(endpoint)
            .await
            .unwrap();
        assert_eq!(mock.hits(), 1);
    }

    /// An endpoint that asks for no early expiry must still behave as it did
    /// when `cache_ttl` alone decided. The body is one the rule above calls a
    /// miss, so a shortened default would show up on the second request.
    #[tokio::test]
    async fn an_endpoint_with_no_rule_holds_its_ttl() {
        let server = httpmock::MockServer::start();
        let (mock, client, probe) = probe(&server, "no-rule", serde_json::json!([]));
        let endpoint = probe.with_cache(NEVER);

        client
            .execute(endpoint.clone())
            .await
            .unwrap();
        elapse().await;
        client
            .execute(endpoint)
            .await
            .unwrap();
        assert_eq!(mock.hits(), 1);
    }
}
