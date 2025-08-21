// use std::error::Error;

use std::collections::HashMap;
use std::sync::Arc;
//use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
// use tokio::time::Duration;
use eyre::Result;
use http::request::Builder as RequestBuilder;
use http::{HeaderMap, HeaderValue, Response};
use http_cache_reqwest::{
   CACacheManager, Cache, CacheMode, HttpCache, HttpCacheOptions,
};
use reqwest::Method;
use reqwest::Request;
use reqwest_middleware::{ClientBuilder as MwClientBuilder, ClientWithMiddleware};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::sync::mpsc::channel;
use url::Url;

use super::QueryParams;
use super::error::ApiError;

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Client {
    async fn request(
        &self,
        method: Method,
        path: Option<String>,
        headers: Option<HeaderMap>,
        query: Option<&QueryParams>,
        body: Option<String>,
    ) -> Result<reqwest::Response>;
    // ) -> Result<()>;
}

#[derive(Clone)]
pub struct RestClient {
    pub client: ClientWithMiddleware,
    pub baseurl: url::Url,
    // auth: Option<String>,
    pub headers: HeaderMap,
    // timeout: Duration,
    // send_null_body: bool,
    // body_wash_fn: fn(String) -> String,
}

impl fmt::Debug for RestClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RestClient")
            .field("baseurl", &self.baseurl)
            .finish()
    }
}

#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
impl Client for RestClient {
    // #[cfg(not(target_arch = "wasm32"))]
    async fn request(
        &self,
        method: Method,
        path: Option<String>,
        headers: Option<HeaderMap>,
        query: Option<&QueryParams>,
        body: Option<String>,
        //  body: Option<HashMap<&str, String>>,
    ) -> Result<reqwest::Response> {
        let req = self.make_request(method, path, headers, query, body)?;
        self.execute_request(req).await
    }
}

pub struct Builder {
    client: Option<ClientWithMiddleware>,
}

impl Builder {
    pub fn with_client(mut self, client: ClientWithMiddleware) -> Self {
        self.client = Some(client);
        self
    }

    /// Create `RestClient` with the configuration in this builder
    pub fn build(self, url: &str) -> Result<RestClient> {
        RestClient::with_builder(url, self)
    }
}

impl Default for Builder {
    fn default() -> Self {
        Self { client: None }
    }
}

impl RestClient {
    /// Construct new client with default configuration to make HTTP requests.
    ///
    /// Use `Builder` to configure the client.
    pub fn new(url: &str) -> Result<RestClient> {
        RestClient::with_builder(url, RestClient::builder())
    }

    // TODO: make cache location come from config
    pub fn with_cache(url: &str) -> Result<RestClient> {
        let inner = reqwest::ClientBuilder::new().build().unwrap();
        let client = MwClientBuilder::new(inner)
             .with(Cache(HttpCache {
                 mode: CacheMode::Default,
                 manager: CACacheManager::new("/tmp/ccache".into(), true),
                 options: HttpCacheOptions::default(),
                //  options: HttpCacheOptions {
                //      cache_mode_fn: Some(Arc::new(|_: &http_cache_reqwest::Parts| {
                //          CacheMode::IgnoreRules
                //      })),
                //      ..Default::default()
                //  },
             }))
            .build();
        let baseurl = Url::parse(url)?;

        Ok(RestClient {
            client,
            baseurl,
            headers: HeaderMap::new(),
        })
    }

    fn build_client() -> ClientWithMiddleware {
        let inner = reqwest::ClientBuilder::new().build().unwrap();
        MwClientBuilder::new(inner)
            //  .with(Cache(HttpCache {
            //      mode: CacheMode::Default,
            //      manager: CACacheManager::new("/tmp/ccache".into(), true),
            //      options: HttpCacheOptions {
            //          cache_mode_fn: Some(Arc::new(|_: &http_cache_reqwest::Parts| {
            //              CacheMode::IgnoreRules
            //          })),
            //          ..Default::default()
            //      },
            //  }))
            .build()
    }

    fn with_builder(url: &str, builder: Builder) -> Result<RestClient> {
        let client = match builder.client {
            Some(client) => client,
            None => Self::build_client(),
        };

        let baseurl = Url::parse(url)?;

        Ok(RestClient {
            client,
            baseurl,
            headers: HeaderMap::new(),
        })
    }

    /// Configure a client
    pub fn builder() -> Builder {
        Builder::default()
    }

    // Add header to the client
    pub fn header(mut self, name: &'static str, value: &str) -> RestClient {
        let value = HeaderValue::from_str(value).unwrap();
        self.headers.insert(name, value);
        self
    }

    async fn execute_request(&self, req: Request) -> Result<reqwest::Response> {
        // reqwest::Response::news}
        Ok(self.client.execute(req).await?)
    }

    fn make_request(
        &self,
        method: Method,
        path: Option<String>,
        headers: Option<HeaderMap>,
        query: Option<&QueryParams>,
        body: Option<String>,
    ) -> Result<Request> {
        let mut build = self
            .client
            .clone()
            .request(method, self.make_uri(path, query)?);
        build = build
            .headers(self.headers.clone())
            // .timeout(Duration::from_secs(60))
            .header("Content-Type", "application/json")
            .header(
                reqwest::header::USER_AGENT,
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) \
         AppleWebKit/537.36 (KHTML, like Gecko) \
         Chrome/115.0.0.0 Safari/537.36",
            )
            .header("ACCEPT", "application/json");

        if headers.is_some() {
            build = build.headers(headers.unwrap());
        }

        // Removed the following line:
        // if query.is_some() {
        //     // let wut = query.unwrap().params;
        //     build = build.query(&query.unwrap().params);
        // }

        if body.is_some() {
            build = build.body(body.unwrap());
        }

        // req = build.build().unwrap();
        let req = build.build()?;
        Ok(req)
    }

    fn make_uri(
        &self,
        path: Option<String>,
        query: Option<&QueryParams>,
    ) -> Result<url::Url> {
        let mut url = self.baseurl.clone();

        if let Some(path) = path {
            let path = path.trim_start_matches('/');
            url = url.join(&path)?;
        }

        // we already encode in another layer as we want finer control over the query params
        if let Some(query) = query {
            let raw = query
                .params
                .iter()
                .map(|(k, v)| format!("{}={}", k, v))
                .collect::<Vec<_>>()
                .join("&");
            url.set_query(Some(&raw));
        };

        url = url.as_str().parse::<url::Url>()?;

        Ok(url)
    }
}
