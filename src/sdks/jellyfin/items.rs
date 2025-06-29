use crate::sdks::core::params::FormParams;
use crate::sdks::core::CommaSeparatedList;
use crate::sdks::core::Endpoint;
use crate::sdks::core::QueryParams;
use crate::media;
use derive_builder::Builder;
use http::HeaderMap;
use http::{header, Method, Request};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;

use super::MediaType;

#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
pub struct ItemsEndpoint {
    #[builder(default = "50")]
    limit: u32,
    #[builder(default = "0")]
    start_index: u32,
    #[builder(default)]
    any_provider_id_equals: Option<Vec<String>>,
    #[builder(default = "true")]
    recursive: bool,
    #[builder(default = "Some(vec![\"Movie\".to_string(),\"Series\".to_string()])")]
    include_item_types: Option<Vec<String>>,
    /// TODO: Should be an enum
    #[builder(
        default = "Some(vec![\"ProviderIds\".to_string(),\"MediaStreams\".to_string()])"
    )]
    fields: Option<Vec<String>>,
}

impl ItemsEndpoint {
    /// Create a builder for the endpoint.
    pub fn builder() -> ItemsEndpointBuilder {
        ItemsEndpointBuilder::default()
    }
}

impl Endpoint for ItemsEndpoint {
    type Output = super::PaginatedResult<Item>;

    fn endpoint(&self) -> String {
        "Items".to_string()
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();
        params
            .push("Recursive", self.recursive.clone())
            .push("Limit", self.limit)
            .push("StartIndex", self.start_index);
        if self.any_provider_id_equals.is_some() {
            params.push(
                "AnyProviderIdEquals",
                self.any_provider_id_equals
                    .clone()
                    .unwrap()
                    .iter()
                    .join(","),
            );
        }
        if self.include_item_types.is_some() {
            params.push(
                "IncludeItemTypes",
                self.include_item_types.clone().unwrap().iter().join(","),
            );
        }
        if self.fields.is_some() {
            params.push("Fields", self.fields.clone().unwrap().iter().join(","));
        }

        params
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Item {
    // #[serde(rename = "Name")]
    pub name: String,
    // #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Type")]
    pub media_type: media::MediaType, // this should be a local type
    pub provider_ids: Option<super::ProviderIds>,
    pub bitrate: Option<u64>,
    #[serde(default = "Vec::new")]
    pub media_streams: Vec<MediaStream>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    pub bitrate: Option<u64>,
    pub display_title: Option<String>,
    pub height: Option<u32>,
    pub width: Option<u32>,
}
