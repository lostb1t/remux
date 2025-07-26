// use std::error::Error;

use std::collections::HashMap;
//use std::time::Duration;

use async_trait::async_trait;
use bytes::Bytes;
// use tokio::time::Duration;
use anyhow::Result;
use dioxus_logger::tracing::{debug, info};
use http::request::Builder as RequestBuilder;
use http::{HeaderMap, HeaderValue, Response};
use reqwest::Method;
use reqwest::Request;
use serde::{Deserialize, Serialize};
use std::sync::mpsc::channel;
use url::Url;

use super::error::ApiError;
use super::QueryParams;

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
    pub client: reqwest::Client,
    pub baseurl: url::Url,
    // auth: Option<String>,
    pub headers: HeaderMap,
    // timeout: Duration,
    // send_null_body: bool,
    // body_wash_fn: fn(String) -> String,
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
    client: Option<reqwest::Client>,
}

impl Builder {
    pub fn with_client(mut self, client: reqwest::Client) -> Self {
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

    fn build_client() -> reqwest::Client {
        reqwest::ClientBuilder::new().build().unwrap()
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

    async fn execute_request(&self, req: Request) -> anyhow::Result<reqwest::Response> {
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
    ) -> Result<Request, anyhow::Error> {
        let mut build = self
            .client
            .clone()
            .request(method, self.make_uri(path, query)?);
        build = build
            .headers(self.headers.clone())
            // .timeout(Duration::from_secs(60))
            .header("Content-Type", "application/json")
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
    ) -> anyhow::Result<url::Url> {
        let mut url = self.baseurl.clone();

        if let Some(path) = path {
            url.set_path([url.path(), &path].join("/").as_str());
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
