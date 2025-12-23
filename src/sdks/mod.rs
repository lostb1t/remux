use axum::http::{HeaderMap, Method};
use serde::de::DeserializeOwned;
use std::sync::Arc;

pub mod aio;
pub mod jellyfin;
pub mod tmdb;

//
// Auth
//

pub trait Auth: Send + Sync {
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
        req.basic_auth(self.username.clone(), Some(self.password.clone()))
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

//
// Errors
//

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
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
    Transport(#[from] reqwest::Error),

    #[error(transparent)]
    Url(#[from] url::ParseError),
}

fn default_error_mapper(status: u16, endpoint: &str, body: &str) -> ApiError {
    if status == 401 {
        ApiError::Unauthorized
    } else {
        ApiError::Http {
            status,
            endpoint: Some(endpoint.to_string()),
            message: "http error".to_string(),
            body: Some(body.to_string()),
        }
    }
}

//
// RestClient
//

#[derive(Clone)]
pub struct RestClient<A: Auth = NoAuth> {
    http: reqwest::Client,
    base: url::Url,
    auth: Arc<A>,
    map_error: fn(u16, &str, &str) -> ApiError,
}

impl RestClient<NoAuth> {
    pub fn new(base: &str) -> Result<Self, url::ParseError> {
        Ok(Self {
            http: reqwest::Client::new(),
            base: url::Url::parse(base)?,
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

    pub fn with_error_mapper(mut self, f: fn(u16, &str, &str) -> ApiError) -> Self {
        self.map_error = f;
        self
    }

    pub async fn execute<EP: Endpoint>(&self, ep: &EP) -> Result<EP::Output, ApiError> {
    let endpoint = ep.path();

    let mut url = self.base.join(&endpoint)?;

    let query = ep.query();
    if !query.is_empty() {
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    }

    let mut req = self.http.request(ep.method(), url).headers(ep.headers());
    req = self.auth.apply(req);

    req = match ep.body() {
        Body::Empty => req,
        Body::Json(v) => req.json(&v),
        Body::Form(v) => req.form(&v),
        Body::Text(s) => req.body(s),
        Body::Bytes(b) => req.body(b),
    };

    let resp = req.send().await.map_err(ApiError::Transport)?;
    let status = resp.status().as_u16();
    let text = resp.text().await.unwrap_or_default();

    match status {
        401 => Err(ApiError::Unauthorized),

        s if (200..300).contains(&s) && text.trim().is_empty() => {
            serde_json::from_str::<EP::Output>("null").map_err(|e| ApiError::Json {
                status: s,
                source: e,
                endpoint: Some(endpoint),
                body: None,
            })
        }

        // Success with body
        s if (200..300).contains(&s) => {
            serde_json::from_str::<EP::Output>(&text).map_err(|e| ApiError::Json {
                status: s,
                source: e,
                endpoint: Some(endpoint),
                body: Some(text),
            })
        }

        // HTTP error
        s => Err(ApiError::Http {
            status: s,
            message: "http error".into(),
            endpoint: Some(endpoint),
            body: Some(text),
        }),
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

