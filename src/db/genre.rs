use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use strum_macros::{Display, EnumIter, EnumString};

#[derive(
    Copy,
    Clone,
    Debug,
    Eq,
    PartialEq,
    Hash,
    Serialize,
    Deserialize,
    DeriveActiveEnum,
    EnumIter,
    EnumString,
    Display,
)]
#[serde(rename_all = "PascalCase")]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "lowercase"
)]
pub enum Genre {
    #[serde(rename = "Action")]
    Action,

    #[serde(rename = "Adventure")]
    Adventure,

    #[serde(rename = "Action & Adventure")]
    #[strum(serialize = "Action & Adventure")]
    ActionAndAdventure,

    #[serde(rename = "Animation")]
    Animation,

    #[serde(rename = "Comedy")]
    Comedy,

    #[serde(rename = "Crime")]
    Crime,

    #[serde(rename = "Documentary")]
    Documentary,

    #[serde(rename = "Drama")]
    Drama,

    #[serde(rename = "Family")]
    Family,

    #[serde(rename = "Fantasy")]
    Fantasy,

    #[serde(rename = "Science Fiction & Fantasy")]
    SciFiAndFantasy,

    #[serde(rename = "History")]
    History,

    #[serde(rename = "Horror")]
    Horror,

    #[serde(rename = "Music")]
    Music,

    #[serde(rename = "Mystery")]
    Mystery,

    #[serde(rename = "Romance")]
    Romance,

    #[serde(rename = "TV Movie")]
    TVMovie,

    #[serde(rename = "Thriller")]
    Thriller,

    #[serde(rename = "War")]
    War,

    #[serde(rename = "War & Politics")]
    WarAndPolitics,

    #[serde(rename = "Western")]
    Western,

    #[serde(rename = "Kids")]
    Kids,

    #[serde(rename = "News")]
    News,

    #[serde(rename = "Reality")]
    Reality,

    #[serde(rename = "Soap")]
    Soap,

    #[serde(rename = "Talk")]
    Talk,
}
