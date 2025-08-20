// use crate::media;
use crate::db::media;
use crate::sdks::core::RestClient;
use serde::{Deserialize, Deserializer, Serialize};
use strum_macros::{Display, EnumString};
pub mod movie;
pub use movie::*;
pub mod show;
pub use show::*;
//pub mod providers;
//pub mod show;
//pub mod trending;
pub mod image;
pub use image::*;

pub type TmdbClient = RestClient;

#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedResult<T> {
    pub page: i64,
    pub results: Vec<T>,
    #[serde(rename = "total_pages")]
    pub total_pages: i64,
    #[serde(rename = "total_results")]
    pub total_results: i64,
}

impl<T> IntoIterator for PaginatedResult<T> {
    type Item = T;
    type IntoIter = std::vec::IntoIter<Self::Item>;

    fn into_iter(self) -> Self::IntoIter {
        self.results.into_iter()
    }
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct MediaShort {
    pub id: u64,
    //pub title: String,
    pub media_type: media::MediaType,
}

#[derive(Debug, Clone, Serialize, EnumString, Display)]
#[strum(serialize_all = "snake_case")]
pub enum MediaType {
    Movie,
    Tv,
}

#[derive(
    strum_macros::Display,
    strum_macros::EnumString,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    serde::Deserialize,
    serde::Serialize,
)]
pub enum Status {
    #[serde(rename = "Rumored")]
    Rumored,
    #[serde(rename = "Planned")]
    Planned,
    #[serde(rename = "In Production")]
    InProduction,
    #[serde(rename = "Post Production")]
    PostProduction,
    #[serde(rename = "Released")]
    Released,
    #[serde(rename = "Canceled")]
    Canceled,
    #[serde(rename = "Pilot")]
    Pilot,
    #[serde(rename = "Returning Series")]
    ReturningSeries,
    #[serde(rename = "Ended")]
    Ended,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct ExternalIds {
    pub imdb_id: Option<String>,
}
