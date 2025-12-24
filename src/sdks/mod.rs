use axum::http::{header, HeaderMap, HeaderValue, Method};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use tracing::{info};
pub mod aio;
pub mod jellyfin;
pub mod tmdb;

//
// Auth
//

pub trait Auth: Send + Sync {
    fn apply(&self, req: reqwest_middleware::RequestBuilder) -> reqwest_middleware::RequestBuilder;
}

#[derive(Clone, Debug)]
pub struct NoAuth;

impl Auth for NoAuth {
    fn apply(&self, req: reqwest_middleware::RequestBuilder) -> reqwest_middleware::RequestBuilder {
        req
    }
}

#[derive(Clone, Debug)]
pub struct BasicAuth {
    pub username: String,
    pub password: String,
}

impl Auth for BasicAuth {
    fn apply(&self, req: reqwest_middleware::RequestBuilder) -> reqwest_middleware::RequestBuilder {
        req.basic_auth(self.username.clone(), Some(self.password.clone()))
    }
}

#[derive(Clone, Debug)]
pub struct BearerAuth {
    pub token: String,
}

impl Auth for BearerAuth {
    fn apply(&self, req: reqwest_middleware::RequestBuilder) -> reqwest_middleware::RequestBuilder {
        req.bearer_auth(&self.token)
    }
}

//
// Errors
//

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("unauthorized")]
    Unauthorized,

    #[error("api error (status={status}) endpoint={endpoint:?}: {message}")]
    Http {
        status: u16,
        message: String,
        endpoint: Option<String>,
        body: Option<String>,
    },

    #[error("json error (status={status}) endpoint={endpoint:?}: {source}")]
    Json {
        status: u16,
        #[source]
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
  }



fn default_error_mapper(status: u16, endpoint: &str, body: &str) -> ClientError {
    if status == 401 {
        ClientError::Unauthorized
    } else {
        ClientError::Http {
            status,
            endpoint: Some(endpoint.to_string()),
            message: "http error".to_string(),
            body: Some(body.to_string()),
        }
    }
}

//
// Body / Endpoint
//

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
    type Output: DeserializeOwned;

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

    /// Per-endpoint cache override.
    ///
    /// Examples:
    /// - Manifest: `Some(CacheMode::ForceCache)`
    /// - Normal endpoints: `None` (use middleware default)
    fn cache_mode(&self) -> Option<http_cache_reqwest::CacheMode> {
        None
    }
}

//
// RestClient (reqwest-middleware + http-cache-reqwest)
//

use http_cache_reqwest::{Cache, CacheMode, HttpCache, HttpCacheOptions, MokaManager};
use reqwest_middleware::{ClientBuilder, ClientWithMiddleware};

#[derive(Clone)]
pub struct RestClient<A: Auth = NoAuth> {
    http: ClientWithMiddleware,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ClientError,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        let inner = reqwest::Client::new();
        let manager = MokaManager::default();

        
        let http = ClientBuilder::new(inner)
            .with(Cache(HttpCache {
                mode: CacheMode::Default,
                manager,
                options: HttpCacheOptions::default(),
            }))
            .build();

        Ok(Self {
            http,
            base: url::Url::parse(format!("{}/", base.trim_start_matches('/')).as_str())?,
            auth: Arc::new(NoAuth),
            map_error: default_error_mapper,
        })
    }
}

impl<A: Auth> RestClient<A> {
    pub fn with_auth<B: Auth>(self, auth: B) -> RestClient<B> {
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

    pub async fn execute<EP: Endpoint>(&self, ep: &EP) -> Result<EP::Output, ClientError> {
        self.execute_inner(ep, None).await
    }

    pub async fn execute_no_cache<EP: Endpoint>(&self, ep: &EP) -> Result<EP::Output, ClientError> {
        self.execute_inner(ep, Some(CacheMode::NoStore)).await
    }

    async fn execute_inner<EP: Endpoint>(
        &self,
        ep: &EP,
        cache_override: Option<CacheMode>,
    ) -> Result<EP::Output, ClientError> {
        let endpoint = ep.path();
        let mut url = self.base.join(&endpoint.trim_start_matches('/'))?;

        let query = ep.query();
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
        }

        let mut req = self.http.request(ep.method(), url.clone()).headers(ep.headers());
        req = self.auth.apply(req);

        let mode = cache_override.or_else(|| ep.cache_mode());
        if let Some(mode) = mode {
            req = req.with_extension(mode);
        }

        req = match ep.body() {
            Body::Empty => req,

            Body::Json(v) => {
                let bytes = serde_json::to_vec(&v).map_err(|e| ClientError::Json {
                    status: 0,
                    source: e,
                    endpoint: Some(url.clone().to_string()),
                    body: Some(v.to_string()),
                })?;

                req.header(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))
                    .body(bytes)
            }

            Body::Form(v) => {
                // Produces: key=a&key=b style
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
                serde_json::from_str::<EP::Output>(&text).map_err(|e| ClientError::Json {
                    status: s,
                    source: e,
                    endpoint: Some(url.clone().to_string()),
                    body: Some(text),
                })
            }

            s => Err((self.map_error)(s, &url.clone().to_string(), &text)),
        }
    }
}

// helpers

use std::borrow::Cow;
use std::fmt;
use std::iter;
use std::ops;

use itertools::Itertools;

/// A comma-separated list of values.
#[derive(Debug, Clone, Default)]
pub struct CommaSeparatedList<T> {
    data: Vec<T>,
}

impl<T> CommaSeparatedList<T> {
    /// Create a new, empty comma-separated list.
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


