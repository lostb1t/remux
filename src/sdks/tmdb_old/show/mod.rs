use serde::{Deserialize, Deserializer, Serialize};

pub mod details;
pub mod discover;
pub use self::details::ShowEndpoint;
pub use self::discover::ShowDiscover;
use super::{Status, ExternalIds};
use serde_with::{serde_as, DisplayFromStr};
use chrono::NaiveDate;

#[serde_as]
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Series {
    pub adult: bool,
    pub backdrop_path: Option<String>,
    //pub created_by: Option<Vec<Creator>>,
    // pub episode_run_time: Vec<u32>,
    #[serde_as(as = "Option<DisplayFromStr>")]
    pub first_air_date: Option<NaiveDate>,
    //pub genres: Vec<Genre>,
    pub homepage: Option<String>,
    pub id: i64,
    //pub in_production: bool,
    // pub languages: Vec<String>,
    pub last_air_date: Option<String>,
    // pub last_episode_to_air: Option<Episode>,
    pub name: String,
    //pub next_episode_to_air: Option<Episode>,
    //pub networks: Option<Vec<Network>>,
    //  pub number_of_episodes: u32,
    // pub number_of_seasons: u32,
    // pub origin_country: Vec<String>,
    pub original_language: String,
    pub original_name: String,
    pub overview: Option<String>,
    pub popularity: f64,
    pub poster_path: Option<String>,
    // pub production_companies: Vec<ProductionCompany>,
    // pub production_countries: Vec<ProductionCountry>,
    pub seasons: Vec<Season>,
    // pub spoken_languages: Vec<SpokenLanguage>,
    pub status: Option<Status>,
    pub tagline: Option<String>,
    pub r#type: String,
    pub vote_average: f64,
    pub vote_count: u32,
    pub external_ids: Option<super::ExternalIds>,
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
