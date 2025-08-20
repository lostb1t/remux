use async_trait::async_trait;
use http::Uri;
use serde::de::DeserializeOwned;
use url::Url;

use super::{ApiError, Client, Endpoint};

pub fn url_to_http_uri(url: Url) -> Uri {
    url.as_str()
        .parse::<Uri>()
        .expect("failed to parse a url::Url as an http::Uri")
}

// pub trait Query<T, C>
// where
//     E: Endpoint + Sync,
//     C: Client,
// {

// #[async_trait]
// pub trait Query<T, C>
// where
//     C: Client,
// {

//     async fn query(&self, client: &C) -> anyhow::Result<T>;
// }
