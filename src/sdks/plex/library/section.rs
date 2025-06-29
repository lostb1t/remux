use crate::clients::core::CommaSeparatedList;
use crate::clients::core::Endpoint;
use crate::clients::core::QueryParams;
use derive_builder::Builder;
use http::HeaderMap;
use http::{header, Method, Request};
use itertools::Itertools;
use serde::Deserialize;
use serde::Serialize;
use std::borrow::Cow;

// #[derive(Debug, Builder, Clone)]
// #[builder(setter(into))]
// pub struct LibrarySectionMedia {
//     section_id: u32,
//     #[builder(default = "50")]
//     limit: u32,
//     #[builder(default = "0")]
//     offset: u32,
//     #[builder(default)]
//     guids: Option<Vec<String>>,
// }

// impl LibrarySectionMedia {
//     /// Create a builder for the endpoint.
//     pub fn builder() -> LibrarySectionMediaBuilder {
//         LibrarySectionMediaBuilder::default()
//     }
// }

// impl Endpoint for LibrarySectionMedia {
//     type Output = crate::clients::plex::models::Root;

//     fn endpoint(&self) -> String {
//         format!("library/sections/{}/all", self.section_id.clone())
//     }

//     fn headers(&self) -> HeaderMap {
//         let mut headers = HeaderMap::new();
//         headers.insert(
//             "x-plex-container-size",
//             self.limit.to_string().parse().unwrap(),
//         );
//         headers.insert(
//             "x-plex-container-start",
//             self.offset.to_string().parse().unwrap(),
//         );
//         headers
//     }

//     fn parameters(&self) -> QueryParams {
//         let mut params = QueryParams::default();
//         params.push("includeGuids", "1");

//         if self.guids.is_some() {
//             params.push("guids", self.guids.clone().unwrap().iter().join(","));
//         }

//         params
//     }
// }

#[derive(Debug, Builder, Clone)]
#[builder(setter(into))]
pub struct LibrarySections {
    #[builder(default = "true")]
    include_preferences: bool,
}

impl LibrarySections {
    /// Create a builder for the endpoint.
    pub fn builder() -> LibrarySectionsBuilder {
        LibrarySectionsBuilder::default()
    }
}

impl Endpoint for LibrarySections {
    type Output = Sections;

    fn endpoint(&self) -> String {
        format!("library/sections/all")
    }
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sections {
    #[serde(rename = "MediaContainer")]
    pub media_container: MediaContainer,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaContainer {
    pub size: i64,
    pub allow_sync: bool,
    pub title1: String,
    #[serde(rename = "Directory")]
    pub metadata: Vec<Section>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Section {
    pub allow_sync: bool,
    pub art: String,
    pub composite: String,
    pub filters: bool,
    pub refreshing: bool,
    pub thumb: String,
    pub key: String,
    #[serde(rename = "type")]
    pub type_field: String,
    pub title: String,
    pub agent: String,
    pub scanner: String,
    pub language: String,
    pub uuid: String,
    pub updated_at: i64,
    pub created_at: i64,
    pub scanned_at: i64,
    pub content: bool,
    pub directory: bool,
    pub content_changed_at: i64,
    pub hidden: i64,
    #[serde(rename = "Location")]
    pub location: Vec<Location>,
    #[serde(rename = "Preferences")]
    pub preferences: Option<Preferences>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub id: i64,
    pub path: String,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Preferences {
    #[serde(rename = "Setting")]
    pub setting: Vec<Setting>,
}

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Setting {
    pub id: String,
    pub label: String,
    pub summary: String,
    #[serde(rename = "type")]
    pub type_field: String,
    pub default: String,
    pub value: String,
    pub hidden: bool,
    pub advanced: bool,
    pub group: String,
    pub enum_values: Option<String>,
}
