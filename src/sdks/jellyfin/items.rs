use super::{BaseItemDto, ItemType};
use crate::media;
use crate::sdks::core::{CommaSeparatedList, Endpoint, QueryParams};
use bon::Builder;
use dioxus_logger::tracing::{debug, info};
use http::{header, HeaderMap, Method, Request};
use itertools::Itertools;
use serde::{Deserialize, Serialize};
use serde_json;
use serde_with::skip_serializing_none;
use strum_macros::Display as EnumDisplay;
use strum_macros::EnumString;
//use serde_with;
extern crate serde_qs;

#[derive(Debug, Builder, Clone)]
pub struct ToggleFavoriteEndpoint {
    pub user_id: String,
    pub item_id: String,
    pub is_favorite: bool,
}

impl Endpoint for ToggleFavoriteEndpoint {
    type Output = ();

    fn endpoint(&self) -> String {
        format!("/Users/{}/FavoriteItems/{}", self.user_id, self.item_id)
    }

    fn method(&self) -> Method {
        if self.is_favorite {
            Method::POST
        } else {
            Method::DELETE
        }
    }
}

#[derive(Debug, Builder, Clone)]
pub struct TogglePlayedEndpoint {
    pub user_id: String,
    pub item_id: String,
    pub is_played: bool,
}

impl Endpoint for TogglePlayedEndpoint {
    type Output = ();

    fn endpoint(&self) -> String {
        format!("/Users/{}/PlayedItems/{}", self.user_id, self.item_id)
    }

    fn method(&self) -> Method {
        if self.is_played {
            Method::POST
        } else {
            Method::DELETE
        }
    }
}

#[derive(EnumString, EnumDisplay, Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "PascalCase")]
pub enum ItemFilter {
    IsPlayed,
    IsUnplayed,
    IsResumable,
    IsFolder,
    IsNotFolder,
    IsLocked,
    IsMissing,
    IsNew,
    IsPlayedRecently,
    IsFavorite,
}

use serde::Serializer;

pub fn comma_separated_option<S, T>(
    value: &Option<Vec<T>>,
    serializer: S,
) -> Result<S::Ok, S::Error>
where
    S: Serializer,
    T: ToString,
{
    let s = value
        .iter()
        .flatten()
        .map(|x| x.to_string())
        .collect::<Vec<_>>()
        .join(",");
    serializer.serialize_str(&s)
}

#[skip_serializing_none]
#[derive(Builder, Default, Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsEndpoint {
    #[builder(default = 25)]
    pub limit: u32,

    #[builder(default = 0)]
    pub start_index: u32,

    #[builder(default)]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub any_provider_id_equals: Vec<String>,

    #[builder(default = true)]
    pub recursive: bool,

    #[builder(default = vec![ItemType::Movie, ItemType::Series])]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub include_item_types: Vec<ItemType>,

    #[builder(default = vec![
        "ProviderIds".to_string(),
        "Genres".to_string(),
        "overview".to_string(),
        "mediaSources".to_string(),
    ])]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub fields: Vec<String>,

    #[builder(default)]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub filters: Vec<ItemFilter>,

    #[builder(default)]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub ids: Vec<String>,

    // #[builder(default)]
    pub parent_id: Option<String>,

    #[serde(skip)]
    pub search_term: Option<String>,
    pub name_starts_with: Option<String>,

    #[builder(default)]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub genres: Vec<String>,

    pub sort_by: Option<super::ItemSortBy>,
    pub sort_order: Option<super::SortOrder>,
}

impl Endpoint for ItemsEndpoint {
    type Output = super::PaginatedResult<super::BaseItemDto>;

    fn endpoint(&self) -> String {
        "Items".to_string()
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::from(self);
        // we override because jellyfin cant handle encoded search string
        if let Some(term) = &self.search_term {
            params.push("SearchTerm", term);
        }
        params
    }
}

#[skip_serializing_none]
#[derive(Builder, Serialize, Default, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsFiltersEndpoint {
    #[builder(default = 25)]
    pub limit: u32,

    #[builder(default = 0)]
    pub start_index: u32,

    //#[builder(default = true)]
    // pub recursive: bool,
    #[builder(default = vec![ItemType::Movie, ItemType::Series])]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub include_item_types: Vec<ItemType>,

    //#[builder(default)]
    //   pub parent_id: Option<Vec<String>>,
    pub search_term: Option<String>,

    pub user_id: Option<String>,
}

impl Endpoint for ItemsFiltersEndpoint {
    type Output = FiltersResponse;

    fn endpoint(&self) -> String {
        "Items/Filters".to_string()
    }

    fn parameters(&self) -> QueryParams {
        self.into()
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FiltersResponse {
    pub genres: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(rename = "OfficialRatings")]
    pub official_ratings: Option<Vec<String>>,
    #[serde(rename = "Years")]
    pub years: Option<Vec<i32>>,
    pub studios: Option<Vec<String>>,
    pub artists: Option<Vec<String>>,
    pub albums: Option<Vec<String>>,
    #[serde(rename = "GenresGrouped")]
    pub genres_grouped: Option<serde_json::Value>,
    #[serde(rename = "TagsGrouped")]
    pub tags_grouped: Option<serde_json::Value>,
}

#[skip_serializing_none]
#[derive(Builder, Default, Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ItemsLatestEndpoint {
    #[builder(default = 25)]
    pub limit: u32,

    #[builder(default = 0)]
    pub start_index: u32,

    #[builder(default = true)]
    pub recursive: bool,

    #[builder(default = vec![ItemType::Movie, ItemType::Series])]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub include_item_types: Vec<ItemType>,

    #[builder(default = vec![
        "ProviderIds".to_string(),
        "Genres".to_string(),
        "overview".to_string(),
    ])]
    #[serde(with = "serde_qs::helpers::comma_separated")]
    pub fields: Vec<String>,
}

impl Endpoint for ItemsLatestEndpoint {
    type Output = super::PaginatedResult<super::BaseItemDto>;

    fn endpoint(&self) -> String {
        "Items/Latest".to_string()
    }

    fn parameters(&self) -> QueryParams {
        self.into()
    }
}

#[skip_serializing_none]
#[derive(Debug, Builder, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfo {
    #[serde(skip)] // used only in URL path
    pub item_id: String,
    //#[serde(skip)]
    pub enable_all_subtitles: Option<bool>,
    pub start_time_ticks: Option<i64>,
    pub max_streaming_bitrate: Option<u64>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub enable_direct_play: Option<bool>,
    pub enable_direct_stream: Option<bool>,
    pub enable_transcoding: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
    pub media_source_id: Option<String>,
    pub device_profile: Option<super::DeviceProfile>,
}

impl Endpoint for PlaybackInfo {
    type Output = PlaybackInfoResponse;

    fn endpoint(&self) -> String {
        format!("/Items/{}/PlaybackInfo", self.item_id)
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn body(&self) -> Option<String> {
        let json = serde_json::to_string(self).ok()?;
        //  info!("{:?}", serde_json::to_string_pretty(self).unwrap());

        Some(json)
    }

    fn parameters(&self) -> QueryParams {
        let mut params = QueryParams::default();

        if let Some(val) = self.enable_all_subtitles {
            params.push("EnableAllSubtitles", val);
        }

        params
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    pub media_sources: Vec<super::MediaSourceInfo>,
    pub play_session_id: Option<String>,
    pub error_code: Option<String>,
    pub playback_start_time_ticks: Option<i64>,
    pub can_seek: Option<bool>,
    pub is_transcoding: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Builder, Default, Clone, Serialize)]
#[serde(rename_all = "PascalCase")]
pub struct VideoStreamRequest {
    #[serde(skip)] // used in the path, not query
    pub item_id: String,

    pub media_source_id: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub container: Option<String>,
    pub transcoding_protocol: Option<String>,
    pub transcoding_container: Option<String>,
    pub max_streaming_bitrate: Option<u64>,
    pub start_time_ticks: Option<i64>,
    pub play_session_id: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub enable_direct_play: Option<bool>,
    pub enable_transcoding: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
    #[serde(rename = "api_key")]
    pub api_key: Option<String>, // needed if not using header auth
    #[serde(rename = "static")]
    pub static_: Option<bool>, // for direct URL
}

impl Endpoint for VideoStreamRequest {
    type Output = PlaybackInfoResponse;

    fn endpoint(&self) -> String {
        format!("/Videos/{}/stream", self.item_id)
    }

    fn method(&self) -> Method {
        Method::GET
    }

    fn parameters(&self) -> QueryParams {
        self.into()
    }
}
