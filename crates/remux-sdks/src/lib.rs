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

    /// How long *this* response should live, for an endpoint that cannot pick a
    /// TTL until it sees one. Only consulted on the write; the read still gates
    /// on `cache_ttl`, so returning `Some` here alone caches nothing.
    fn cache_ttl_for(&self, _response: &Self::Output) -> Option<Duration> {
        self.cache_ttl()
    }

    /// The JSON to deserialize in place of the body `status` came with, for a
    /// status that is an answer rather than a failure. `None` leaves it an
    /// error. See [`Absent`], which is how an endpoint opts in.
    fn absent_body(&self, _status: u16) -> Option<&'static str> {
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
        let resp = req
            .send()
            .await?;
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
                if let Some(ttl) = endpoint.cache_ttl_for(&arc) {
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
            s if endpoint
                .absent_body(s)
                .is_some() =>
            {
                let stand_in = endpoint
                    .absent_body(s)
                    .unwrap_or("null");
                let arc = serde_json::from_str::<EP::Output>(stand_in)
                    .map(Arc::new)
                    .map_err(|e| ClientError::Json {
                        status: s,
                        source: e,
                        endpoint: Some(url.to_string()),
                        body: Some(text.clone()),
                    })?;
                if let Some(ttl) = endpoint.cache_ttl_for(&arc) {
                    HTTP_CACHE.save_arc_with_weight(
                        cache_key,
                        Arc::clone(&arc),
                        stand_in.len() as u32,
                        ttl,
                    );
                }
                Ok(arc)
            }
            s => Err((self.map_error)(s, &url.to_string(), &text)),
        }
    }
}

pub trait OptionalEndpoint: Endpoint + Sized {
    /// Read `status` as "no such thing" rather than a failure, making the
    /// `Output` an `Option`. The answer then caches like any other, so a run of
    /// deliveries naming something the provider does not have asks once.
    fn absent_on(self, status: u16) -> Absent<Self> {
        Absent {
            endpoint: self,
            status,
        }
    }
}

impl<EP: Endpoint + Sized> OptionalEndpoint for EP {}

/// Wraps an endpoint so one status deserializes as `null`. Compose it inside
/// `with_cache` to hold the miss: `.absent_on(404).with_cache(ttl)`.
#[derive(Clone)]
pub struct Absent<EP: Endpoint> {
    endpoint: EP,
    status: u16,
}

impl<EP: Endpoint> Endpoint for Absent<EP> {
    type Output = Option<EP::Output>;

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

    fn cache_ttl_for(&self, response: &Self::Output) -> Option<Duration> {
        match response {
            Some(inner) => self
                .endpoint
                .cache_ttl_for(inner),
            None => self.cache_ttl(),
        }
    }

    fn absent_body(&self, status: u16) -> Option<&'static str> {
        if status == self.status {
            return Some("null");
        }
        self.endpoint
            .absent_body(status)
    }
}

pub trait CachedEndpoint: Endpoint + Sized {
    fn with_cache(self, ttl: Duration) -> Cached<Self> {
        Cached {
            endpoint: self,
            ttl,
            expire_early: None,
        }
    }
}

impl<EP: Endpoint + Sized> CachedEndpoint for EP {}

#[derive(Clone)]
pub struct Cached<EP: Endpoint> {
    endpoint: EP,
    ttl: Duration,
    expire_early: Option<(Duration, fn(&EP::Output) -> bool)>,
}

impl<EP: Endpoint> Cached<EP> {
    /// Keep a response matching `when` for `ttl` instead of the full cache
    /// lifetime. Reads still consult the cache; only the expiry stamped on
    /// the stored entry changes.
    pub fn with_cache_ttl_if(
        self,
        ttl: Duration,
        when: fn(&EP::Output) -> bool,
    ) -> Self {
        Self {
            expire_early: Some((ttl, when)),
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

    fn cache_ttl(&self) -> Option<Duration> {
        Some(self.ttl)
    }

    fn cache_ttl_for(&self, response: &Self::Output) -> Option<Duration> {
        match self.expire_early {
            Some((short, when)) if when(response) => Some(short),
            _ => Some(self.ttl),
        }
    }

    fn absent_body(&self, status: u16) -> Option<&'static str> {
        self.endpoint
            .absent_body(status)
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

    fn cache_ttl(&self) -> Option<Duration> {
        self.endpoint
            .cache_ttl()
    }

    fn cache_ttl_for(&self, response: &Self::Output) -> Option<Duration> {
        self.endpoint
            .cache_ttl_for(response)
    }

    fn absent_body(&self, status: u16) -> Option<&'static str> {
        self.endpoint
            .absent_body(status)
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
            .with_cache_ttl_if(BRIEF, is_empty);

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
            .with_cache_ttl_if(BRIEF, is_empty);

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
