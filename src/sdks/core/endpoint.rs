use anyhow::Result;
use async_trait::async_trait;
use http::{self, header, HeaderMap, Method, Request};
use serde::de::DeserializeOwned;
use std::{borrow::Cow, collections::HashMap};

use super::{query, ApiError, Client, QueryParams, RestClient};

/// A trait for providing the necessary information for a single REST API endpoint.
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
pub trait Endpoint {
    type Output: serde::de::DeserializeOwned;

    // fn output(&self) -> T {
    //     Method::GET
    // }

    /// The HTTP method to use for the endpoint.
    fn method(&self) -> Method {
        Method::GET
    }
    /// The path to the endpoint.
    fn endpoint(&self) -> String;

    /// Query parameters for the endpoint.
    // TODO: This can definitly auto generated with an derive macro: (#param)
    fn parameters(&self) -> QueryParams {
        QueryParams::default()
    }

    /// headers for the endpoint.
    fn headers(&self) -> HeaderMap {
        HeaderMap::new()
    }

    /// The body for the endpoint.
    fn body(&self) -> Option<HashMap<&str, String>> {
        None
    }
    // #[cfg(not(target_arch = "wasm32"))]
    async fn query(&self, client: &RestClient) -> Result<Self::Output> {
        let res = client
            .request(
                self.method(),
                Some(self.endpoint()),
                Some(self.headers()),
                Some(&self.parameters()),
                self.body(),
            )
            .await?;
        let status = res.status();
        if !status.is_success() {
            return Err(anyhow::anyhow!(res.text().await.unwrap()));
        }
        let result = res.json::<Self::Output>().await?;
        Ok(result)
    }

    // async fn query<T: serde::de::DeserializeOwned>(&self, client: &RestClient) -> Result<T> {
    //     let res = client.request(
    //         self.method(),
    //         Some(self.endpoint()),
    //         Some(self.headers()),
    //         Some(&self.parameters()),
    //         self.body()
    //     ).await?;
    //     let status = res.status();
    //     if !status.is_success() {
    //         return Err(anyhow::anyhow!(res.text().await.unwrap()));
    //     }
    //     let result = res.json::<T>().await?;
    //     Ok(result)
    // }
}

// #[cfg_attr(not(target_arch = "wasm32"), async_trait)]
// #[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
// pub trait PageableEndpoint: Endpoint + super::pagination::Pageable {
//     // type Output: serde::de::DeserializeOwned;
//     // type Item;
// }

// #[async_trait]
// impl<E, T, C> Query<T, C> for E
// where
//     E: Endpoint + Sync,
//     T: DeserializeOwned + 'static,
//     C: Client + Sync,
// {
//     // #[cfg(target_arch = "wasm32")]
//     // async fn query(&self, client: C) -> Result<T> {
//     //     let res = client.request(
//     //         self.method(),
//     //         Some(self.endpoint()),
//     //         Some(self.headers()),
//     //         Some(&self.parameters()),
//     //         self.body()
//     //     ).await?;
//     //     // let status = res.status();
//     //     // if !status.is_success() {
//     //     //     return Err(anyhow::anyhow!(res.text().await.unwrap()));
//     //     // }
//     //     // let result = res.json::<T>().await?;
//     //     let result: T = serde_json::from_str("{ \"Id\": 1, \"Name\": \"Coke\" }").unwrap();
//     //     Ok(result)
//     // }

//     // #[cfg(not(target_arch = "wasm32"))]
//     async fn query(&self, client: &C) -> Result<T> {
//         let res = client.request(
//             self.method(),
//             Some(self.endpoint()),
//             Some(self.headers()),
//             Some(&self.parameters()),
//             self.body()
//         ).await?;
//         let status = res.status();
//         if !status.is_success() {
//             return Err(anyhow::anyhow!(res.text().await.unwrap()));
//         }
//         let result = res.json::<T>().await?;
//         Ok(result)
//     }
// }
