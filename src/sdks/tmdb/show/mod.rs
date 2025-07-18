use serde::{Deserialize, Deserializer, Serialize};

pub mod details;
pub mod discover;
pub use self::details::ShowEndpoint;
pub use self::discover::ShowDiscover;
pub use super::providers::WatchProviderResult;

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ShowBase {
    pub id: u64,
    pub name: String,
    pub original_name: String,
    pub original_language: String,
    pub overview: String,
    //#[serde(default, with = "crate::util::optional_date")]
    //pub release_date: Option<chrono::NaiveDate>,
    pub poster_path: Option<String>,
    pub backdrop_path: Option<String>,
    pub adult: bool,
    pub popularity: f64,
    pub vote_count: u64,
    pub vote_average: f64,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ShowShort {
    #[serde(flatten)]
    pub inner: ShowBase,
    pub genre_ids: Option<Vec<u64>>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct Show {
    #[serde(flatten)]
    pub inner: ShowBase,
    //pub budget: u64,
    // pub genres: Vec<Genre>,
    //#[serde(deserialize_with = "crate::util::empty_string::deserialize")]
    //pub homepage: Option<String>,
    //#[serde(deserialize_with = "crate::util::empty_string::deserialize")]
    //pub imdb_id: Option<String>,
    //pub belongs_to_collection: Option<CollectionBase>,
    //pub production_companies: Vec<CompanyShort>,
    //pub production_countries: Vec<Country>,
    //pub revenue: u64,
    //pub runtime: Option<u64>,
    // pub spoken_languages: Vec<Language>,
    //pub status: Status,
    pub tagline: Option<String>,
    #[serde(default = "Images::default")]
    pub images: Images,
    #[serde(rename = "watch/providers", default = "WatchProviderResult::default")]
    pub watch_providers: WatchProviderResult,
}

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
