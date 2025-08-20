use serde::{Deserialize, Deserializer, Serialize};

pub mod details;
pub mod discover;
pub use self::details::MovieEndpoint;
pub use self::discover::MovieDiscover;
use serde_with::{serde_as, DisplayFromStr};
use chrono::NaiveDate;
// use crate::sdks::{Endpoint, QueryParams, Status};
use super::{Status, ExternalIds};
// pub use super::providers::WatchProviderResult;

#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Movie {
    pub id: i64,
    pub title: String,
    pub overview: Option<String>,
    pub release_date: Option<NaiveDate>,
    pub runtime: Option<i64>,
    pub vote_average: Option<f32>,
    pub vote_count: Option<i64>,
    pub adult: bool,
    pub status: Option<Status>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub imdb_id: Option<String>,
    pub original_language: String,
    pub external_ids: Option<ExternalIds>,
}


// #[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
// pub struct MovieBase {
//     pub id: u64,
//     pub title: String,
//     pub original_title: String,
//     pub original_language: String,
//     pub overview: String,
//     //#[serde(default, with = "crate::util::optional_date")]
//     //pub release_date: Option<chrono::NaiveDate>,
//     pub poster_path: Option<String>,
//     pub backdrop_path: Option<String>,
//     pub adult: bool,
//     pub popularity: f64,
//     pub vote_count: u64,
//     pub vote_average: f64,
//     pub video: bool,
// }

// #[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
// pub struct MovieShort {
//     #[serde(flatten)]
//     pub inner: MovieBase,
//     pub genre_ids: Option<Vec<u64>>,
// }

// #[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
// pub struct Movie {
//     #[serde(flatten)]
//     pub inner: MovieBase,
//     pub budget: u64,
//     // pub genres: Vec<Genre>,
//     //#[serde(deserialize_with = "crate::util::empty_string::deserialize")]
//     //pub homepage: Option<String>,
//     //#[serde(deserialize_with = "crate::util::empty_string::deserialize")]
//     //pub imdb_id: Option<String>,
//     //pub belongs_to_collection: Option<CollectionBase>,
//     //pub production_companies: Vec<CompanyShort>,
//     //pub production_countries: Vec<Country>,
//     pub revenue: u64,
//     //pub runtime: Option<u64>,
//     // pub spoken_languages: Vec<Language>,
//     //pub status: Status,
//     pub tagline: Option<String>,
//     #[serde(default = "Images::default")]
//     pub images: Images,
//     #[serde(rename = "watch/providers", default = "WatchProviderResult::default")]
//     pub watch_providers: WatchProviderResult,
// }

#[derive(Clone, Default, Debug, PartialEq, Deserialize, Serialize)]
pub struct Images {
    #[serde(default = "Vec::new")]
    pub backdrops: Vec<Image>,
    #[serde(default = "Vec::new")]
    pub posters: Vec<Image>,
    #[serde(default = "Vec::new")]
    pub logos: Vec<Image>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Image {
    pub file_path: String,
    pub iso_639_1: Option<String>,
}
