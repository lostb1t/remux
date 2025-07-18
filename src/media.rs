use chrono::DateTime;
use chrono::Utc;
use ordered_float::OrderedFloat;
use std::collections::HashMap;
use std::fmt::{self, Display};

// use crate::plex::{self, *};
// use crate::clients::core::query::Query;

// use gluesql::core::executor::Payload;
use dioxus_logger::tracing::*;
use serde::{Deserialize, Serialize};
// use plex_api::library::{self as plex_library, MetadataItem};
// use plex_api::media_container::server::library::Guid as PlexApiGuid;
// use plex_api::media_container::server::library::Metadata as PlexApiMetadata;
// use plex_api::media_container::server::library::MetadataMediaContainer;
use crate::components;
use crate::sdks;
use anyhow::{anyhow, Error, Result};
use bon::Builder;
use dioxus::prelude::*;
use serde_json::{json, Value};
use std::hash::{Hash, Hasher};
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;

#[derive(Debug, Clone, PartialEq, Hash, Serialize, Deserialize)]
pub struct Rating {
    pub source: RatingSource,
    pub score: u32, // Changed to u32 for simplicity. scale of 0 - 100
}

impl Rating {
    pub fn format_score(&self) -> String {
        match self.source {
            RatingSource::RottenTomatoes => self.score.to_string(),
            _ => ((self.score as f32) / 10.0).round().to_string(),
        }
        // debug!("Rating: {:?}", self);
    }

    pub fn icon_path(&self) -> String {
        match self.source {
            RatingSource::RottenTomatoes => {
                if self.score < 50 {
                    asset!("/assets/img/icon/rt-rotten.png").into()
                } else {
                    asset!("/assets/img/icon/rt.svg").into()
                }
            }
            RatingSource::TMDb => asset!("/assets/img/icon/tmdb.svg").into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, EnumString, EnumDisplay)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum RatingSource {
    RottenTomatoes,
    //   Metacritic,
    //  IMDb,
    TMDb,
    // Custom,
}

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

// #[derive(PartialEq, Eq, Hash, Clone, Debug, Serialize, Deserialize, EnumString, EnumDisplay)]
// #[serde(rename_all = "snake_case")]
// #[strum(serialize_all = "snake_case", ascii_case_insensitive)]
// pub enum Rating {
//     Rotten({icon: String, score: f32}),
//     Rotten({icon: String, score: f32}),
// }

#[derive(PartialEq, Eq, Clone, Hash, Debug, Serialize, Deserialize, EnumString, EnumDisplay)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum MediaType {
    #[strum(to_string = "Movie")]
    #[serde(alias = "movie", alias = "Movie")]
    Movie,
    #[strum(serialize = "tv", to_string = "TV")]
    #[serde(
        alias = "show",
        alias = "Show",
        alias = "tv",
        alias = "TV",
        alias = "Series"
    )]
    Series,
    #[strum(to_string = "season")]
    Season,
    #[strum(to_string = "episode")]
    Episode,
    #[strum(to_string = "catalog")]
    Catalog,
}

impl Default for MediaType {
    fn default() -> Self {
        MediaType::Movie
    }
}

use std::convert::TryFrom;

impl TryFrom<sdks::jellyfin::ItemType> for MediaType {
    type Error = Error;

    fn try_from(kind: sdks::jellyfin::ItemType) -> Result<Self, Self::Error> {
        match kind {
            sdks::jellyfin::ItemType::Movie => Ok(MediaType::Movie),
            sdks::jellyfin::ItemType::Series => Ok(MediaType::Series),
            sdks::jellyfin::ItemType::Season => Ok(MediaType::Season),
            sdks::jellyfin::ItemType::Episode => Ok(MediaType::Episode),
            sdks::jellyfin::ItemType::BoxSet => Ok(MediaType::Catalog),
            _ => Err(anyhow!("Unsupported ItemType: {:?}", kind)),
        }
    }
}

impl TryFrom<MediaType> for sdks::jellyfin::ItemType {
    type Error = Error;

    fn try_from(value: MediaType) -> Result<Self, Self::Error> {
        match value {
            MediaType::Movie => Ok(sdks::jellyfin::ItemType::Movie),
            MediaType::Series => Ok(sdks::jellyfin::ItemType::Series),
            MediaType::Season => Ok(sdks::jellyfin::ItemType::Season),
            MediaType::Episode => Ok(sdks::jellyfin::ItemType::Episode),
            // MediaType::Collection => Ok(sdks::jellyfin::ItemType::BoxSet),
            _ => Err(anyhow!("Unsupported MediaType: {:?}", value)),
        }
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
#[derive(PartialEq, Default, Hash, Clone, Debug, Serialize, Deserialize)]
// #[builder(setter(into))]
pub struct ExternalIds {
    // tmdb is always required
    pub tmdb: Option<u32>,
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

#[derive(
    Copy,
    Serialize,
    Debug,
    Clone,
    Eq,
    PartialEq,
    Deserialize,
    Hash,
    strum_macros::Display,
    strum_macros::EnumString,
    // Default,
)]
#[serde(rename_all = "PascalCase")]
pub enum MediaStreamType {
    Audio,
    Video,
    Subtitle,
    EmbeddedImage,
    Data,
    Lyric,
}

impl From<sdks::jellyfin::MediaStreamType> for MediaStreamType {
    fn from(t: sdks::jellyfin::MediaStreamType) -> Self {
        match t {
            sdks::jellyfin::MediaStreamType::Audio => MediaStreamType::Audio,
            sdks::jellyfin::MediaStreamType::Video => MediaStreamType::Video,
            sdks::jellyfin::MediaStreamType::Subtitle => MediaStreamType::Subtitle,
            sdks::jellyfin::MediaStreamType::EmbeddedImage => MediaStreamType::EmbeddedImage,
            sdks::jellyfin::MediaStreamType::Data => MediaStreamType::Data,
            sdks::jellyfin::MediaStreamType::Lyric => MediaStreamType::Lyric,
        }
    }
}

#[derive(Builder, Serialize, Hash, PartialEq, Deserialize, Debug, Clone)]
pub struct MediaSource {
    pub id: String,
    pub name: String,
    // pub transcoding_url: Option<String>,
    pub media_streams: Option<Vec<MediaStream>>,
}

impl TryFrom<sdks::jellyfin::MediaSourceInfo> for MediaSource {
    type Error = Error;

    fn try_from(info: sdks::jellyfin::MediaSourceInfo) -> Result<Self, Self::Error> {
        Ok(MediaSource {
            id: info
                .id
                .ok_or_else(|| anyhow!("Missing MediaSourceInfo.id"))?,
            name: info.name.unwrap_or_default(),
            media_streams: info
                .media_streams
                .map(|streams| {
                    streams
                        .into_iter()
                        .map(MediaStream::try_from)
                        .collect::<Result<Vec<_>, _>>()
                })
                .transpose()?,
        })
    }
}

#[derive(Builder, Serialize, Hash, PartialEq, Deserialize, Debug, Clone)]
pub struct MediaStream {
    pub index: Option<i32>,
    pub title: Option<String>,
    pub display_title: Option<String>,
    pub type_: Option<MediaStreamType>,
}

impl TryFrom<sdks::jellyfin::MediaStream> for MediaStream {
    type Error = Error;

    fn try_from(stream: sdks::jellyfin::MediaStream) -> Result<Self, Self::Error> {
        Ok(MediaStream {
            index: stream.index,
            title: stream.title,
            display_title: stream.display_title,
            type_: stream.type_.map(Into::into),
        })
    }
}

#[derive(Debug, Clone, Hash, Copy)]
pub enum ImageType {
    Poster,
    Backdrop,
    Thumb,
    Logo,
}

#[derive(Default, PartialEq, Hash, Clone, Debug)]
//#[builder(setter(into))]
pub struct Genre {
    pub id: String,
    pub name: String,
}

impl TryFrom<sdks::jellyfin::BaseItemDto> for Genre {
    type Error = anyhow::Error;

    fn try_from(item: sdks::jellyfin::BaseItemDto) -> anyhow::Result<Self, Self::Error> {
        Ok(Genre {
            id: item.id.unwrap(),
            name: item.name.unwrap(),
        })
    }
}

// #[derive(Debug, Clone, Hash, PartialEq, Serialize, Deserialize)]
// #[serde(rename_all = "PascalCase")]
// pub struct Ratings {
//     pub playback_position_ticks: i64,
//     pub play_count: i32,
//     pub is_favorite: bool,
//     pub is_watched: bool,
// }

#[derive(Debug, Clone, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserData {
    pub playback_position_ticks: i64,
    pub play_count: i32,
    pub is_favorite: bool,
    pub is_watched: bool,
    //  pub key: String,
    //  pub item_id: String,
}

impl From<sdks::jellyfin::UserItemDataDto> for UserData {
    fn from(t: sdks::jellyfin::UserItemDataDto) -> Self {
        UserData {
            playback_position_ticks: t.playback_position_ticks,
            play_count: t.play_count,
            is_favorite: t.is_favorite,
            is_watched: t.played,
        }
    }
}

// TODO: lazy load images: https://stackoverflow.com/questions/59683330/implementing-a-lazy-load-in-by-way-of-enum-type-in-rust#59685305
#[derive(Builder, Default, Hash, Serialize, Deserialize, PartialEq, Clone, Debug)]
//#[builder(setter(into))]
pub struct Media {
    pub id: String,
    pub title: String,
    pub media_type: MediaType,
    pub external_ids: ExternalIds,
    // #[builder(default = None)]
    pub description: Option<String>,
    pub release_date: Option<DateTime<Utc>>,
    pub runtime_seconds: Option<i64>,
    pub poster: Option<String>,
    //#[builder(default = "None")]
    pub thumb: Option<String>,
    pub backdrop: Option<String>,
    pub logo: Option<String>,
    pub genres: Vec<String>,
    pub index_number: Option<i32>,
    pub user_data: Option<UserData>,
    pub official_rating: Option<String>,
    #[builder(default)]
    pub card_variant: components::CardVariant,
    #[builder(default)]
    pub ratings: Vec<Rating>,
    #[builder(default)]
    pub media_sources: Vec<MediaSource>,

    // for catalogs/collections
    #[builder(default = true)]
    pub enabled: bool,
}
use chrono::Duration;
impl Media {
    pub fn is_series(&self) -> bool {
        self.media_type == MediaType::Series
    }

    pub fn formatted_runtime(&self) -> String {
        // debug!("Formatting runtime: {seconds} seconds");
        let duration = Duration::seconds(self.runtime_seconds.unwrap_or(0));
        let hours = duration.num_hours();
        let minutes = duration.num_minutes() % 60;

        match (hours, minutes) {
            (0, 0) => "0m".to_string(),
            (0, m) => format!("{m}m"),
            (h, 0) => format!("{h}h"),
            (h, m) => format!("{h}h {m}m"),
        }
    }

    pub fn progress(&self) -> Option<u32> {
        if let (Some(data), Some(runtime)) = (&self.user_data, self.runtime_seconds) {
            if data.playback_position_ticks != 0 && runtime != 0 {
                let seconds = data.playback_position_ticks / 10_000_000;
                return Some(((seconds * 100) / runtime).min(100) as u32);
            }
        }
        None
    }
}

impl TryFrom<sdks::jellyfin::BaseItemDto> for Media {
    type Error = anyhow::Error;

    fn try_from(item: sdks::jellyfin::BaseItemDto) -> anyhow::Result<Self, Self::Error> {
        Ok(Media::builder()
            .id(item.id.unwrap().to_string())
            .title(item.name.unwrap())
            .media_type(item.type_.unwrap().try_into().unwrap())
            .external_ids(ExternalIds {
                ..Default::default()
            })
            .maybe_index_number(item.index_number)
            .maybe_description(item.overview)
            .maybe_release_date(item.premiere_date)
            .genres(
                item.genres
                    .unwrap_or_default()
                    .try_into()
                    .expect("Expecting some genres"),
            )
            .media_sources(
                item.media_sources
                    .clone()
                    .unwrap_or_default()
                    .into_iter()
                    .filter_map(|s| s.try_into().ok())
                    .collect(),
            )
            .maybe_poster(item.image_tags.clone().unwrap_or_default().primary)
            .maybe_official_rating(item.official_rating)
            .maybe_backdrop(
                item.backdrop_image_tags
                    .clone()
                    .unwrap_or_default()
                    .first()
                    .cloned(),
            )
            .maybe_runtime_seconds(item.run_time_ticks.map(|ticks| ticks / 10_000_000))
            .maybe_logo(item.image_tags.unwrap().logo)
            .maybe_user_data(item.user_data.map(|x| x.into()))
            .ratings({
                let mut ratings = Vec::new();
                if let Some(rating) = item.critic_rating {
                    // debug!("Rating: {:?}", rating);
                    ratings.push(Rating {
                        source: RatingSource::RottenTomatoes,
                        score: rating as u32,
                    });
                }
                if let Some(rating) = item.community_rating {
                    // dafuq does this come from
                    ratings.push(Rating {
                        source: RatingSource::TMDb,
                        score: (rating * 10.0) as u32,
                    });
                }
                // debug!("Ratings: {:?}", ratings);
                ratings
            })
            .build())
    }
}
