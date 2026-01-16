use axum::http::{HeaderMap, HeaderValue, Method, header};
use itertools::Itertools;
use md5;
use moka::Expiry;
use moka::sync::Cache;
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::any::Any;
use std::fmt;
use std::iter;
use std::ops;
use std::sync::Arc;
use std::time::Duration;
use tracing::*;

use std::sync::LazyLock;

static CACHE: LazyLock<Cache<String, Arc<CachedValue>>> =
    LazyLock::new(|| Cache::builder().max_capacity(50_000).build());

fn hash_key(key: &str) -> String {
    let result = md5::compute(key.as_bytes());
    format!("{:x}", result)
}

#[derive(Debug)]
pub struct CachedValue {
    pub value: String,
    pub ttl: Duration,
}

impl Clone for CachedValue {
    fn clone(&self) -> Self {
        Self {
            value: self.value.clone(),
            ttl: self.ttl,
        }
    }
}

#[derive(Clone, Default)]
struct PerEntryExpiry;

impl Expiry<String, Arc<CachedValue>> for PerEntryExpiry {
    fn expire_after_create(
        &self,
        _key: &String,
        value: &Arc<CachedValue>,
        _current_time: std::time::Instant,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_update(
        &self,
        _key: &String,
        value: &Arc<CachedValue>,
        _current_time: std::time::Instant,
        _current_duration: Option<Duration>,
    ) -> Option<Duration> {
        Some(value.ttl)
    }

    fn expire_after_read(
        &self,
        _key: &String,
        _value: &Arc<CachedValue>,
        _current_time: std::time::Instant,
        current_duration: Option<Duration>,
        _now: std::time::Instant,
    ) -> Option<Duration> {
        current_duration
    }
}

pub mod aio;
pub mod tmdb;

pub trait Auth: Send + Sync + Clone {
    fn apply(
        &self,
        req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder;
}

#[derive(Clone, Debug)]
pub struct NoAuth;

impl Auth for NoAuth {
    fn apply(
        &self,
        req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        req
    }
}

#[derive(Clone, Debug)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl Auth for BasicAuth {
    fn apply(
        &self,
        req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        req.basic_auth(&self.username, Some(&self.password))
    }
}

#[derive(Clone, Debug)]
pub struct BearerAuth {
    pub token: String,
}

impl Auth for BearerAuth {
    fn apply(
        &self,
        req: reqwest_middleware::RequestBuilder,
    ) -> reqwest_middleware::RequestBuilder {
        req.bearer_auth(&self.token)
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
    Transport(#[from] reqwest_middleware::Error),
    #[error(transparent)]
    Url(#[from] url::ParseError),
    #[error(transparent)]
    UrlEncoded(#[from] serde_urlencoded::ser::Error),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
    #[error(transparent)]
    JsonDeserialize(#[from] serde_json::Error),
}

fn default_error_mapper(status: u16, endpoint: &str, body: &str) -> ClientError {
    if status == 401 {
        ClientError::Unauthorized
    } else {
        ClientError::Http {
            status,
            message: "http error".to_string(),
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
    http: ClientWithMiddleware,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ClientError,
    cache: Cache<String, Arc<CachedValue>>,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        let http = ClientBuilder::new(reqwest::Client::new()).build();
        let cache = CACHE.clone();
        Ok(Self {
            http,
            base: url::Url::parse(
                format!("{}/", base.trim_start_matches('/')).as_str(),
            )?,
            auth: Arc::new(NoAuth),
            map_error: default_error_mapper,
            cache,
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
            cache: self.cache,
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
        let mut url = self.base.join(&path.trim_start_matches('/')).unwrap();
        let query = endpoint.query();
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }
        let cache_key = hash_key(&url.to_string());

        if let Some(ttl) = endpoint.cache_ttl() {
            if let Some(cached) = self.cache.get(&cache_key) {
                return Ok(serde_json::from_str(&cached.value)?);
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
                    endpoint: Some(url.clone().to_string()),
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
               

              let result = Ok(serde_json::from_str::<EP::Output>(&text)?);
              
                if let Some(ttl) = endpoint.cache_ttl() {
                    //if let Ok(ref value) = result {
                        let cached_value = Arc::new(CachedValue {
                            value: text.clone(),
                            ttl,
                        });

                        self.cache.insert(cache_key, cached_value);
                   // }
                }
                result
            }
            s => Err((self.map_error)(s, &url.clone().to_string(), &text)),
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

#[tokio::test]
async fn test_media_metadata_caching() {
    use serde::{Deserialize, Serialize};
    use std::time::Duration;

    let client =
        Arc::new(RestClient::new("https://your-media-server-api.com").unwrap());

    #[derive(Debug, Clone, Serialize, Deserialize)]
    struct MovieMetadata {
        pub id: String,
        pub title: String,
        pub year: u32,
    }

    #[derive(Clone)]
    struct MovieEndpoint {
        pub id: String,
    }

    impl Endpoint for MovieEndpoint {
        type Output = MovieMetadata;

        fn path(&self) -> String {
            format!("movies/{}", self.id)
        }

        fn cache_ttl(&self) -> Option<Duration> {
            Some(Duration::from_secs(60))
        }
    }

    // First request (should hit the API)
    let endpoint = MovieEndpoint {
        id: "tt1234567".to_string(),
    };
    let metadata1 = client.execute(endpoint.clone()).await.unwrap();
    println!("First request metadata: {:?}", metadata1);

    // Second request (should hit the cache)
    let metadata2 = client.execute(endpoint).await.unwrap();
    println!("Second request metadata: {:?}", metadata2);

    assert_eq!(metadata1.title, metadata2.title);
    assert_eq!(metadata1.year, metadata2.year);
}
