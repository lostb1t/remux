use serde::{Deserialize, Deserializer, Serialize};

use anyhow::Result;
use derive_builder::Builder;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

use crate::sdks;
use crate::sdks::core::Endpoint;
use crate::sdks::core::RestClient;
use crate::media::Media;
use crate::media::MediaType;
use std::collections::HashMap;

#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
pub struct WatchProviderList {
    #[builder(default)]
    id: u32,
    #[builder(default)]
    media_type: MediaType,
}

impl WatchProviderList {
    /// Create a builder for the endpoint.
    pub fn builder() -> WatchProviderListBuilder {
        WatchProviderListBuilder::default()
    }
}

impl crate::sdks::core::Endpoint for WatchProviderList {
    type Output = Vec<WatchProviderResult>;

    fn endpoint(&self) -> String {
        format!(
            "{}/{}/watch/providers",
            self.media_type.clone(),
            self.id.clone()
        )
        .to_string()
    }

    fn parameters(&self) -> crate::sdks::core::QueryParams {
        let mut params = crate::sdks::core::QueryParams::default();
        //params.push("page", self.page.clone());
        //params.push("sort_by", self.sort_by.clone().to_string());
        params
    }
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct WatchProvider {
    pub provider_id: u64,
    pub provider_name: String,
    pub display_priority: u64,
    pub logo_path: String,
}

#[derive(Clone, PartialEq, Debug, Deserialize, Serialize)]
pub struct LocatedWatchProvider {
    pub link: String,
    #[serde(default)]
    pub flatrate: Vec<WatchProvider>,
    #[serde(default)]
    pub rent: Vec<WatchProvider>,
    #[serde(default)]
    pub buy: Vec<WatchProvider>,
}

#[derive(Clone, Default, PartialEq, Debug, Deserialize, Serialize)]
pub struct WatchProviderResult {
    pub id: Option<u64>,
    pub results: HashMap<String, LocatedWatchProvider>,
}
