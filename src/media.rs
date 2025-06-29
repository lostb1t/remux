use std::collections::HashMap;
use std::fmt::{self, Display};

// use crate::plex::{self, *};
// use crate::clients::core::query::Query;

// use gluesql::core::executor::Payload;
use serde::{Deserialize, Serialize};
// use plex_api::library::{self as plex_library, MetadataItem};
// use plex_api::media_container::server::library::Guid as PlexApiGuid;
// use plex_api::media_container::server::library::Metadata as PlexApiMetadata;
// use plex_api::media_container::server::library::MetadataMediaContainer;

use anyhow::Result;
use bon::Builder;
use serde_json::{json, Error, Value};
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

#[derive(Debug, Clone, PartialEq, EnumDisplay)]
pub enum Guid {
    // Local(String),
    Tmdb(String),
    Plex(String),
    #[cfg(not(feature = "tests_deny_unknown_fields"))]
    Unknown(String),
}

// #[derive(Clone, Debug)]
// pub enum MediaSource {
//     Plex(SourceMedia),
//     Jellyfin(SourceMedia),
//     // PlexMetaProvider(SourceMedia),
//     // TmdbMetaProvider(SourceMedia),
//     // TextPlex(Value),
//     // Local,
// }

// impl PartialEq for MediaSource {
//     fn eq(&self, other: &Self) -> bool {
//         dbg!("starting equal");
//         match self {
//             MediaSource::Plex(_) => {
//                 dbg!("plex left match");
//                 match other {
//                     MediaSource::Plex(_) => true,
//                     _ => false,
//                 }
//             }
//             _ => self == other,
//         }
//     }
// }

// impl PartialEq for MediaSource {
//     fn eq(&self, other: &Self) -> bool {
//         dbg!("starting equal");
//         match self {
//             MediaSource::Plex(_) => {
//                 dbg!("plex left match");
//                 match other {
//                     MediaSource::Plex(_) => true,
//                     _ => false,
//                 }
//             },
//             _ => self == other,
//         }
//     }
// }

#[derive(PartialEq, Eq, Clone, Debug, Serialize, Deserialize, EnumString, EnumDisplay)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum MediaType {
    // #[strum(serialize = "Sup my buddies")]
    #[serde(alias = "movie", alias = "Movie")]
    Movie,
    #[strum(serialize = "tv")]
    #[serde(
        alias = "show",
        alias = "Show",
        alias = "tv",
        alias = "TV",
        alias = "Series"
    )]
    Show,
    //  #[strum(to_string = "season")]
    //  Season,
    //  #[strum(to_string = "episode")]
    //  Episode,
}

impl Default for MediaType {
    fn default() -> Self {
        MediaType::Movie
    }
}

// impl PlexMediaType {
//     fn value(&self) -> i32 {
//         match *self {
//             PlexMediaType::Movie => 1,
//             PlexMediaType::Tv => 2,
//             PlexMediaType::Season => 3,
//             PlexMediaType::Episode => 4,
//         }
//     }
// }

pub fn placeholder_image(width: u32, height: u32) -> Option<String> {
    Some(format!("https://placehold.co/{}x{}", height, width))
}

// #[derive(PartialEq, Clone, Debug, Default, Queryable, Selectable)]
// #[diesel(table_name = crate::schema::media)]
// #[diesel(check_for_backend(diesel::sqlite::Sqlite))]
#[derive(PartialEq, Clone, Debug, Serialize, Deserialize)]
// #[builder(setter(into))]
pub struct ExternalIds {
    // tmdb is always required
    pub tmdb: u32,
}

// #[derive(Debug, PartialEq, Clone)]
// pub enum ImageType {
//     Backdrop,
//     BackdropText,
//     Hero,
//     Poster,
// }

// #[derive(Debug, PartialEq, Clone)]
// pub struct Img {
//     pub image_type: ImageType,
//     pub original_url: String,
// }

// #[derive(Debug, PartialEq, Clone)]
// pub enum LazyImage {
//     Unresolved(ImageType),
//     Resolved(Img),
// }

// TODO: lazy load images: https://stackoverflow.com/questions/59683330/implementing-a-lazy-load-in-by-way-of-enum-type-in-rust#59685305
#[derive(Builder, PartialEq, Clone, Debug)]
//#[builder(setter(into))]
pub struct Media {
    pub id: String,
    pub title: String,
    pub media_type: MediaType,
    pub external_ids: ExternalIds,
    //#[builder(default = "None")]
    pub description: Option<String>,
    // #[serde(default = "placeholder_image")]
    //#[builder(default = "placeholder_image(600,400)")]
    //pub backdrop: Option<String>,
    // #[serde(default = "placeholder_image")]
    // #[builder(default = "placeholder_image(300,500)")]
    //#[builder(default = "None")]
    pub poster: Option<String>,
    //#[builder(default = "None")]
    pub landscape: Option<String>,
    pub backdrop: Option<String>,
    // #[serde(default = "placeholder_image")]
    //#[builder(default = "placeholder_image(600,400)")]
    //pub hero: Option<String>,
    // #[serde(default = "placeholder_image")]
    // #[builder(default = "Some(\"https://placehold.co/600x400\".to_string())")]
    //#[builder(default = "placeholder_image(600,400)")]
    //#[builder(default = "None")]
    //pub logo: Option<String>,
}

impl Media {}
