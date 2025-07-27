use crate::media;
use crate::sdks::core::RestClient;
use serde::{Deserialize, Deserializer, Serialize};
use strum_macros::{Display, EnumString};

//pub mod movie;
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
