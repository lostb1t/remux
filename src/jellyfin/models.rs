use axum::routing::get_service;
use merge::Merge;
use std::str::FromStr;
use std::{sync::Arc, time::Duration};
//use progenitor::generate_api;
use crate::aio::AioService;
use crate::db;
use anyhow::anyhow;
use chrono::{DateTime, Utc};
use serde_aux::prelude::*;
use serde_with::skip_serializing_none;
use std::collections::HashMap;
use std::collections::HashSet;
use uuid::Uuid;
//generate_api!(
//    spec = "src/sdks/jellyfin/openapi.json", // The OpenAPI document
//    interface = Builder
//);

use crate::sdks::aio;
use crate::utils::{get_uuid, server_id};
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_alias::serde_alias;
use serde_with::formats::CommaSeparator;
use serde_with::{DisplayFromStr, StringWithSeparator, serde_as};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateUserByName {
    //#[serde(alias = "Password")]
    pub pw: String,
    pub username: String,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub access_token: Option<String>,
    pub server_id: String,
    //pub session_info: Option<SessionInfoDto>,
    pub user: Option<UserDto>,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    pub id: Option<String>,
    pub local_address: Option<String>,
    pub operating_system: Option<String>,
    pub product_name: Option<String>,
    pub server_name: Option<String>,
    pub startup_wizard_completed: Option<bool>,
    pub version: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub operating_system_display_name: Option<String>,
    pub has_pending_restart: Option<bool>,
    pub is_shutting_down: Option<bool>,
    pub supports_library_monitor: Option<bool>,
    pub web_socket_port_number: Option<u16>,
    pub completed_installations: Option<Vec<String>>,
    pub can_self_restart: Option<bool>,
    pub can_launch_web_browser: Option<bool>,
    pub program_data_path: Option<String>,
    pub web_path: Option<String>,
    pub items_by_name_path: Option<String>,
    pub cache_path: Option<String>,
    pub log_path: Option<String>,
    pub internal_metadata_path: Option<String>,
    pub transcoding_temp_path: Option<String>,
    pub has_update_available: Option<bool>,
    pub encoder_location: Option<String>,
    pub system_architecture: Option<String>,
    pub local_address: Option<String>,
    pub server_name: Option<String>,
    pub version: Option<String>,
    pub operating_system: Option<String>,
    pub id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualFolderInfo {
    /// The name of the virtual folder.
    pub name: String,
    /// The locations (paths) associated with the virtual folder.
    // pub locations: Vec<String>,
    /// The type of the collection.
    pub collection_type: Option<CollectionTypeOptions>,
    /// Library-specific options.
    // pub library_options: LibraryOptions,
    /// The item identifier.
    pub item_id: Option<String>,
    /// The primary image item identifier.
    pub primary_image_item_id: Option<String>,
    /// Progress of the refresh operation (0.0 to 1.0).
    pub refresh_progress: Option<f64>,
    /// The status of the refresh operation.
    pub refresh_status: Option<String>,
}

// Placeholder for CollectionTypeOptions and LibraryOptions.
// You'll need to define these according to your needs.
#[derive(Debug, Serialize, Deserialize)]
pub enum CollectionTypeOptions {
    // Define your variants here
}

#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LibraryOptions {
    // Define your fields here
}

#[derive(Debug, Serialize, Deserialize)]
pub struct SpecialViewOptionDto {
    pub name: Option<String>,
    pub id: Option<String>,
}

// ordering of macros is very important. keep as is
#[serde_alias(
    CamelCase,
    PascalCase,
    LowerCase,
    UpperCase,
    SnakeCase,
    ScreamingSnakeCase,
    KebabCase,
    ScreamingKebabCase
)]
#[serde_as]
#[derive(default2::Default, Debug, Deserialize, Clone)]
#[skip_serializing_none]
pub struct GetItemsQuery {
    pub user_id: Option<Uuid>,
    pub max_official_rating: Option<String>,
    pub has_theme_song: Option<bool>,
    pub has_theme_video: Option<bool>,
    pub has_subtitles: Option<bool>,
    pub has_special_feature: Option<bool>,
    pub has_trailer: Option<bool>,
    pub adjacent_to: Option<String>,
    pub index_number: Option<i64>,
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
    pub search_term: Option<String>,
    pub parent_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    // #[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, ItemFields>>")]
    //#[serde_as(as = "Option<StringWithSeparator<CommaSeparator, ItemFields>>")]
    #[serde(deserialize_with = "deserialize_fields", default)]
    pub fields: Option<Vec<ItemFields>>,
    pub exclude_item_types: Option<Vec<MediaType>>,
    pub include_item_types: Option<Vec<MediaType>>,
    pub is_favorite: Option<bool>,
    pub image_type_limit: Option<i64>,
    pub enable_image_types: Option<Vec<String>>,
    pub name_starts_with_or_greater: Option<String>,
    pub name_starts_with: Option<String>,
    pub name_less_than: Option<String>,
    //#[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, ItemSortBy>>")]
    //pub sort_by: Option<Vec<ItemSortBy>>,
    //#[serde_as(as = "Option<StringWithSeparator::<CommaSeparator, SortOrder>>")]
    //pub sort_order: Option<SortOrder>,
    pub enable_images: Option<bool>,
    // #[default(true)]
    pub enable_user_data: Option<bool>,
    pub enable_total_record_count: Option<bool>,
    pub enable_resumable: Option<bool>,
    pub enable_rewatching: Option<bool>,
    pub disable_first_episode: Option<bool>,
    pub next_up_date_cutoff: Option<String>,
    pub years: Option<Vec<i64>>,
    pub genres: Option<Vec<String>>,
    pub genre_ids: Option<Vec<String>>,
    pub official_ratings: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub media_types: Option<Vec<String>>,
    pub filters: Option<Vec<ItemFilter>>,
    pub person_ids: Option<Vec<String>>,
    pub person_types: Option<Vec<String>>,
    pub studios: Option<Vec<String>>,
    pub studio_ids: Option<Vec<String>>,
    pub exclude_artist_ids: Option<Vec<String>>,
    pub ids: Option<Vec<Uuid>>,
}

impl GetItemsQuery {
    pub fn get_requested_item_types(&self) -> Vec<MediaType> {
        let mut requested: Vec<MediaType> = vec![MediaType::Movie, MediaType::Series];

        if let Some(include_types) = &self.include_item_types {
            requested = include_types
                .iter()
                .filter(|t| matches!(t, MediaType::Movie | MediaType::Series))
                .cloned()
                .collect();
        }

        if let Some(exclude_types) = &self.exclude_item_types {
            requested.retain(|t| !exclude_types.contains(t));
        }

        //if let Some(media_types) = &self.media_types {
        //    if media_types.iter().any(|mt| mt == "Video") {
        //        requested.retain(|t| t != &MediaType::Series);
        //    }
        //}

        requested
    }
}

pub fn deserialize_fields<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<ItemFields>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum FieldInput {
        Single(String),
        Multiple(Vec<String>),
    }

    let input = Option::<FieldInput>::deserialize(deserializer)?;

    let fields = match input {
        Some(FieldInput::Single(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<ItemFields>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        value = %s,
                        error = ?e,
                        "ItemFields parse failed, ignoring value"
                    );
                    None
                }
            })
            .collect::<Vec<_>>(),

        Some(FieldInput::Multiple(ss)) => ss
            .iter()
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<ItemFields>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(
                        value = %s,
                        error = ?e,
                        "ItemFields parse failed, ignoring value"
                    );
                    None
                }
            })
            .collect::<Vec<_>>(),

        None => return Ok(None),
    };

    Ok(Some(fields))
}

#[derive(Default, Debug, Deserialize)]
#[serde_alias(CamelCase, PascalCase)]
pub struct VideoStreamQuery {
    pub container: Option<String>,
    #[serde(rename = "static")]
    pub static_: Option<bool>,
    pub params: Option<String>,
    pub tag: Option<String>,
    pub device_profile_id: Option<String>,
    pub play_session_id: Option<String>,
    pub segment_container: Option<String>,
    pub segment_length: Option<i64>,
    pub min_segments: Option<i64>,
    pub media_source_id: Option<Uuid>,
    pub device_id: Option<String>,
    pub audio_codec: Option<String>,
    pub enable_auto_stream_copy: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
    pub break_on_non_key_frames: Option<bool>,
    pub audio_stream_index: Option<i64>,
    pub video_stream_index: Option<i64>,
    pub context: Option<String>, // Could use enum "Streaming" | "Static"
    pub stream_options: Option<std::collections::HashMap<String, Option<String>>>,
    pub enable_audio_vbr_encoding: Option<bool>,
    pub always_burn_in_subtitle_when_transcoding: Option<bool>,
}

#[serde_alias(CamelCase, PascalCase)]
#[derive(Default, Debug, Deserialize)]
#[serde(default)]
#[serde_as]
pub struct PlaybackInfoQuery {
    pub user_id: Option<String>,
    pub max_streaming_bitrate: Option<i64>,
    pub start_time_ticks: Option<i64>,
    #[serde_as(deserialize_as = "serde_with::DefaultOnError")]
    pub audio_stream_index: Option<i64>,
    #[serde_as(deserialize_as = "serde_with::DefaultOnError")]
    pub subtitle_stream_index: Option<i64>,
    pub max_audio_channels: Option<i64>,
    #[serde_as(deserialize_as = "serde_with::DefaultOnError")]
    pub media_source_id: Option<Uuid>,
    pub live_stream_id: Option<String>,
    pub auto_open_live_stream: Option<bool>,
    pub enable_direct_play: Option<bool>,
    pub enable_direct_stream: Option<bool>,
    pub enable_transcoding: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
    pub always_burn_in_subtitle_when_transcoding: Option<bool>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct ImageQuery {
    pub tag: Option<String>,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDtoQueryResult {
    /// Gets or sets the items.
    #[serde(default)] // Always serialize, even if empty
    pub items: Vec<BaseItemDto>,

    /// Gets or sets the index of the first record in Items.
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_index: u32,

    /// Gets or sets the total number of records available.
    // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_record_count: i64,
}

#[skip_serializing_none]
#[derive(default2::Default, Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceInfo {
    pub analyze_duration_ms: Option<i64>,
    pub bitrate: Option<i64>,
    pub buffer_ms: Option<i64>,
    pub container: Option<String>,
    pub default_audio_stream_index: Option<i64>,
    pub default_subtitle_stream_index: Option<i64>,
    pub e_tag: Option<Uuid>,
    pub encoder_path: Option<String>,
    //  pub encoder_protocol: Option<MediaProtocol>,
    pub fallback_max_streaming_bitrate: Option<i64>,
    pub formats: Option<Vec<String>>,
    pub gen_pts_input: Option<bool>,
    pub has_segments: Option<bool>,
    pub id: Uuid,
    pub ignore_dts: Option<bool>,
    pub ignore_index: Option<bool>,
    pub is_infinite_stream: Option<bool>,
    #[default(Some(false))]
    pub is_remote: Option<bool>,
    //pub iso_type: Option<IsoType>,
    pub live_stream_id: Option<String>,
    //pub media_attachments: Option<Vec<MediaAttachment>>,
    #[default(vec![])]
    pub media_streams: Vec<MediaStream>,
    pub name: Option<String>,
    pub open_token: Option<String>,
    pub path: Option<String>,
    pub protocol: Option<String>,
    pub read_at_native_framerate: Option<bool>,
    //pub required_http_headers: Option<HashMap<String, Option<String>>>,
    pub requires_closing: Option<bool>,
    pub requires_looping: Option<bool>,
    pub requires_opening: Option<bool>,
    pub run_time_ticks: Option<i64>,
    pub size: Option<i64>,
    #[default(Some(true))]
    pub supports_direct_play: Option<bool>,
    #[default(Some(true))]
    pub supports_direct_stream: Option<bool>,
    pub supports_external_stream: Option<bool>,
    #[default(Some(true))]
    pub supports_probing: Option<bool>,
    // TODO: implement
    #[default(Some(false))]
    pub supports_transcoding: Option<bool>,
    //  pub timestamp: Option<TransportStreamTimestamp>,
    pub transcoding_container: Option<String>,
    /// Media streaming protocol.
    /// Lowercase for backwards compatibility.
    //  pub transcoding_sub_protocol: Option<MediaStreamProtocol>,
    pub transcoding_url: Option<String>,
    // pub type_: Option<MediaSourceType>,
    #[default(false)]
    pub use_most_compatible_transcoding_profile: bool,
    //  pub video3_d_format: Option<Video3DFormat>,
    #[default("VideoFile".to_string())]
    pub video_type: String,
}

impl MediaSourceInfo {
    pub fn probe_in_place(&mut self) -> anyhow::Result<()> {
        let path = self.path.clone().ok_or_else(|| anyhow!("missing url"))?;
        let info = ffprobe::ffprobe(path)?;

        let probed: MediaSourceInfo = info.into();

        let id = self.id.clone();
        let name = self.name.clone();
        let path = self.path.clone();

        *self = probed;

        self.id = id;
        self.name = name;
        self.path = path;

        Ok(())
    }
}
impl From<db::Media> for MediaSourceInfo {
    fn from(source: db::Media) -> Self {
        MediaSourceInfo {
            id: source.id.clone(),
            e_tag: Some(source.id.clone()),
            path: source.url,
            protocol: Some("File".to_string()),
            supports_transcoding: Some(false),
            supports_direct_stream: Some(true),
            supports_direct_play: Some(true),
            is_remote: Some(false),
            name: Some(source.title.clone()),
            ..Default::default()
        }
    }
}

#[skip_serializing_none]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    // pub error_code: Option<PlaybackErrorCode>,
    pub media_sources: Vec<MediaSourceInfo>,
    pub play_session_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct QueryFiltersLegacy {
    pub genres: Option<Vec<String>>,
    pub official_ratings: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    pub years: Option<Vec<i64>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserConfiguration {
    pub audio_language_preference: Option<String>,
    pub play_default_audio_track: Option<bool>,
    pub subtitle_language_preference: Option<String>,
    pub display_missing_episodes: Option<bool>,
    pub grouped_folders: Option<Vec<String>>,
    pub subtitle_mode: Option<String>,
    pub display_collections_view: Option<bool>,
    pub enable_local_password: Option<bool>,
    pub ordered_views: Option<Vec<String>>,
    pub latest_items_excludes: Option<Vec<String>>,
    pub my_media_excludes: Option<Vec<String>>,
    pub hide_played_in_latest: Option<bool>,
    pub remember_audio_selections: Option<bool>,
    pub remember_subtitle_selections: Option<bool>,
    pub enable_next_episode_auto_play: Option<bool>,
    pub cast_receiver_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct DisplayPreferencesDto {
    pub id: Option<String>,
    pub view_type: Option<String>,
    pub sort_by: Option<String>,
    pub index_by: Option<String>,
    pub remember_indexing: bool,
    pub primary_image_height: i64,
    pub primary_image_width: i64,
    pub custom_prefs: Option<HashMap<String, Option<String>>>,
    pub scroll_direction: String,
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    pub sort_order: String,
    pub show_sidebar: bool,
    pub client: Option<String>,
}

#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Serialize,
    Deserialize,
)]
#[serde(rename_all = "PascalCase")]
pub enum ScrollDirection {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ImageType {
    Primary,
    Backdrop,
    Logo,
    Thumb,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub configuration: Option<UserConfiguration>,
    pub enable_auto_login: Option<bool>,
    pub has_configured_easy_password: Option<bool>,
    pub has_configured_password: Option<bool>,
    pub has_password: Option<bool>,
    pub id: Uuid,
    pub last_activity_date: Option<DateTime<Utc>>,
    pub last_login_date: Option<DateTime<Utc>>,
    pub name: String,
    pub policy: UserPolicy,
    pub primary_image_aspect_ratio: Option<f64>,
    pub primary_image_tag: Option<String>,
    pub server_id: String,
    pub server_name: Option<String>,
}

impl Default for UserDto {
    fn default() -> Self {
        Self {
            configuration: Some(UserConfiguration::default()),
            enable_auto_login: Some(false),
            has_configured_easy_password: Some(false),
            has_configured_password: Some(true),
            has_password: Some(true),
            id: get_uuid(),
            last_activity_date: None,
            last_login_date: None,
            name: "default".to_string(),
            policy: UserPolicy::default(),
            primary_image_aspect_ratio: None,
            primary_image_tag: None,
            server_id: server_id(),
            server_name: None,
        }
    }
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct UserPolicy {
    pub is_administrator: bool,
    pub is_hidden: Option<bool>,
    pub enable_collection_management: Option<bool>,
    pub enable_subtitle_management: Option<bool>,
    pub enable_lyric_management: Option<bool>,
    pub is_disabled: Option<bool>,
    pub blocked_tags: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    pub enable_user_preference_access: Option<bool>,
    pub access_schedules: Option<Vec<String>>,
    pub block_unrated_items: Option<Vec<String>>,
    pub enable_remote_control_of_other_users: Option<bool>,
    pub enable_shared_device_control: Option<bool>,
    pub enable_remote_access: Option<bool>,
    pub enable_live_tv_management: Option<bool>,
    pub enable_live_tv_access: Option<bool>,
    pub enable_media_playback: Option<bool>,
    pub enable_audio_playback_transcoding: Option<bool>,
    pub enable_video_playback_transcoding: Option<bool>,
    pub enable_playback_remuxing: Option<bool>,
    pub force_remote_source_transcoding: Option<bool>,
    pub enable_content_deletion: Option<bool>,
    pub enable_content_deletion_from_folders: Option<Vec<String>>,
    pub enable_content_downloading: Option<bool>,
    pub enable_sync_transcoding: Option<bool>,
    pub enable_media_conversion: Option<bool>,
    pub enabled_devices: Option<Vec<String>>,
    pub enable_all_devices: Option<bool>,
    pub enabled_channels: Option<Vec<String>>,
    pub enable_all_channels: Option<bool>,
    pub enabled_folders: Option<Vec<String>>,
    pub enable_all_folders: Option<bool>,
    pub invalid_login_attempt_count: Option<i64>,
    pub login_attempts_before_lockout: Option<i64>,
    pub max_active_sessions: Option<i64>,
    pub enable_public_sharing: Option<bool>,
    pub blocked_media_folders: Option<Vec<String>>,
    pub blocked_channels: Option<Vec<String>>,
    pub remote_client_bitrate_limit: Option<i64>,
    pub authentication_provider_id: Option<String>,
    pub password_reset_provider_id: Option<String>,
    pub sync_play_access: Option<String>,
}

impl Default for UserPolicy {
    fn default() -> Self {
        Self {
            access_schedules: None,
            allowed_tags: None,
            authentication_provider_id: Some(
                "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider"
                    .into(),
            ),
            block_unrated_items: None,
            blocked_channels: None,
            blocked_media_folders: None,
            blocked_tags: None,
            enable_all_channels: None,
            enable_all_devices: None,
            enable_all_folders: None,
            enable_audio_playback_transcoding: None,
            enable_collection_management: Some(false),
            enable_content_deletion: None,
            enable_content_deletion_from_folders: None,
            enable_content_downloading: Some(true),
            enable_live_tv_access: None,
            enable_live_tv_management: None,
            enable_lyric_management: Some(false),
            enable_media_conversion: Some(true),
            enable_media_playback: Some(true),
            enable_playback_remuxing: Some(true),
            enable_public_sharing: None,
            enable_remote_access: None,
            enable_remote_control_of_other_users: None,
            enable_shared_device_control: None,
            enable_subtitle_management: Some(false),
            enable_sync_transcoding: None,
            enable_user_preference_access: None,
            enable_video_playback_transcoding: Some(true),
            enabled_channels: None,
            enabled_devices: None,
            enabled_folders: None,
            force_remote_source_transcoding: None,
            invalid_login_attempt_count: None,
            is_administrator: false,
            is_disabled: Some(false),
            is_hidden: Some(true),
            login_attempts_before_lockout: None,
            max_active_sessions: None,
            password_reset_provider_id: Some(
                "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider"
                    .into(),
            ),
            remote_client_bitrate_limit: None,
            sync_play_access: None,
        }
    }
}

#[derive(Default, Deserialize, PartialEq, Serialize, Clone, Debug)]
#[skip_serializing_none]
#[serde(rename_all = "PascalCase")]
pub struct MediaStream {
    pub aspect_ratio: Option<String>,
    //  pub audio_spatial_format: AudioSpatialFormat,
    pub average_frame_rate: Option<f32>,
    pub bit_depth: Option<i64>,
    pub bit_rate: Option<i64>,
    pub bl_present_flag: Option<i64>,
    pub channel_layout: Option<String>,
    pub channels: Option<i64>,
    pub codec: Option<String>,
    pub codec_tag: Option<String>,
    pub codec_time_base: Option<String>,
    pub color_primaries: Option<String>,
    pub color_range: Option<String>,
    pub color_space: Option<String>,
    pub color_transfer: Option<String>,
    pub comment: Option<String>,
    // pub delivery_method: Option<SubtitleDeliveryMethod>,
    pub delivery_url: Option<String>,
    pub display_title: Option<String>,
    pub dv_bl_signal_compatibility_id: Option<i64>,
    pub dv_level: Option<i64>,
    pub dv_profile: Option<i64>,
    pub dv_version_major: Option<i64>,
    pub dv_version_minor: Option<i64>,
    pub el_present_flag: Option<i64>,
    pub height: Option<i64>,
    pub index: Option<i64>,
    pub is_anamorphic: Option<bool>,
    pub is_avc: Option<bool>,
    pub is_default: Option<bool>,
    pub is_external: Option<bool>,
    pub is_external_url: Option<bool>,
    pub is_forced: Option<bool>,
    pub is_hearing_impaired: Option<bool>,
    pub is_interlaced: Option<bool>,
    pub is_text_subtitle_stream: Option<bool>,
    pub language: Option<String>,
    pub level: Option<f64>,
    pub localized_default: Option<String>,
    pub localized_external: Option<String>,
    pub localized_forced: Option<String>,
    pub localized_hearing_impaired: Option<String>,
    pub localized_undefined: Option<String>,
    pub nal_length_size: Option<String>,
    pub packet_length: Option<i64>,
    pub path: Option<String>,
    pub pixel_format: Option<String>,
    pub profile: Option<String>,
    pub real_frame_rate: Option<f32>,
    pub ref_frames: Option<i64>,
    pub reference_frame_rate: Option<f32>,
    pub rotation: Option<i64>,
    pub rpu_present_flag: Option<i64>,
    pub sample_rate: Option<i64>,
    pub score: Option<i64>,
    pub supports_external_stream: Option<bool>,
    pub time_base: Option<String>,
    pub title: Option<String>,
    pub type_: Option<MediaStreamType>,
    pub video_do_vi_title: Option<String>,
    pub video_range: Option<VideoRange>,
    pub video_range_type: Option<VideoRangeType>,
    pub width: Option<i64>,
}

#[skip_serializing_none]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageTags {
    pub primary: Option<String>,
    pub logo: Option<String>,
    pub thumb: Option<String>,
    pub backdrop: Option<String>,
}

#[skip_serializing_none]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ImageBlurHashes {
    pub backdrop: Option<HashMap<String, String>>,
    pub primary: Option<HashMap<String, String>>,
    pub logo: Option<HashMap<String, String>>,
}

// todo: should be an hashmap
#[skip_serializing_none]
#[derive(Default, Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ProviderIds {
    pub imdb: Option<String>,
    pub tmdb: Option<String>,
    pub aio: Option<String>,
}

#[skip_serializing_none]
#[derive(default2::Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub rating: Option<f32>,
    pub played: Option<bool>,
    pub last_played_date: Option<DateTime<Utc>>,
    pub playback_position_ticks: Option<i64>,
    pub play_count: Option<i32>,
    pub is_favorite: Option<bool>,
    pub likes: Option<bool>,
    pub last_liked_date: Option<DateTime<Utc>>,
    pub favorite_added_date: Option<DateTime<Utc>>,
    pub played_percentage: Option<f32>,
    pub last_updated: Option<DateTime<Utc>>,
    pub key: Option<String>,
    // pub item_id: String,
}

#[skip_serializing_none]
#[derive(default2::Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub id: Uuid,
    #[default(server_id())]
    pub server_id: String,
    pub name: Option<String>,
    pub original_title: Option<String>,
    pub original_title_sortable: Option<String>,
    pub etag: Option<Uuid>,
    pub source_type: Option<String>,
    pub playlist_item_id: Option<String>,
    pub date_created: Option<String>,
    pub date_last_media_added: Option<String>,
    pub extra_type: Option<String>,
    pub airs_before_season_number: Option<i64>,
    pub airs_after_season_number: Option<i64>,
    pub airs_before_episode_number: Option<i64>,
    #[default(Some(false))]
    pub can_delete: Option<bool>,
    #[default(Some(true))]
    pub can_download: Option<bool>,
    pub has_subtitles: Option<bool>,
    pub preferred_metadata_language: Option<String>,
    pub preferred_metadata_country_code: Option<String>,
    pub supports_sync: Option<bool>,
    pub container: Option<String>,
    pub sort_name: Option<String>,
    pub forced_sort_name: Option<String>,
    pub video_3d_format: Option<String>,
    //#[serde_as(as = "Option<DisplayFromStr>")]
    pub premiere_date: Option<DateTime<Utc>>,
    //pub external_urls: Option<Vec<ExternalUrl>>,
    pub media_sources: Option<Vec<MediaSourceInfo>>,
    pub critic_rating: Option<f64>,
    pub production_locations: Option<Vec<String>>,
    pub path: Option<String>,
    pub official_rating: Option<String>,
    pub custom_rating: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub overview: Option<String>,
    pub taglines: Option<Vec<String>>,
    pub genres: Option<Vec<String>>,
    pub community_rating: Option<f64>,
    pub cumulative_run_time_ticks: Option<i64>,
    pub run_time_ticks: Option<i64>,
    pub play_access: Option<String>,
    pub aspect_ratio: Option<String>,
    pub production_year: Option<i64>,
    pub is_place_holder: Option<bool>,
    pub number: Option<String>,
    pub channel_number: Option<String>,
    pub index_number: Option<i64>,
    pub index_number_end: Option<i64>,
    pub parent_index_number: Option<i64>,
    pub critic_rating_summary: Option<String>,
    pub is_hd: Option<bool>,
    pub is_folder: bool,
    pub parent_id: Option<Uuid>,
    #[default(MediaType::Movie)]
    pub type_: MediaType,
    // pub people: Option<Vec<BaseItemPerson>>,
    // pub studios: Option<Vec<NameLongIdPair>>,
    //pub genre_items: Option<Vec<NameLongIdPair>>,
    pub parent_logo_item_id: Option<String>,
    pub parent_backdrop_item_id: Option<String>,
    pub parent_backdrop_image_tags: Option<Vec<String>>,
    pub local_trailer_count: Option<i64>,
    pub remote_trailers: Option<Vec<String>>,
    pub user_data: Option<UserItemDataDto>,
    pub recursive_item_count: Option<i64>,
    pub child_count: Option<i64>,
    pub series_name: Option<String>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    pub special_feature_count: Option<i64>,
    pub display_preferences_id: Option<String>,
    pub status: Option<Status>,
    pub air_time: Option<String>,
    pub air_days: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,

    // this is fucking weird. And its used.
    // anyway we set it to poster format by default
    #[default(0.6)]
    pub primary_image_aspect_ratio: f32,
    //pub artists: Option<Vec<String>>,
    //pub artist_items: Option<Vec<NameIdPair>>,
    //pub album: Option<String>,
    pub collection_type: Option<CollectionType>,
    pub display_order: Option<String>,
    pub album_id: Option<String>,
    pub album_primary_image_tag: Option<String>,
    pub series_primary_image_tag: Option<String>,
    //pub album_artist: Option<String>,
    //pub album_artists: Option<Vec<NameIdPair>>,
    pub season_name: Option<String>,
    pub media_streams: Option<Vec<MediaStream>>,
    pub video_type: Option<String>,
    pub part_count: Option<i64>,
    pub media_source_count: Option<i64>,
    pub image_tags: Option<ImageTags>,
    pub backdrop_image_tags: Option<Vec<String>>,
    pub image_blur_hashes: Option<ImageBlurHashes>,
    pub screenshot_image_tags: Option<Vec<String>>,
    pub parent_thumb_item_id: Option<String>,
    pub parent_thumb_image_tag: Option<String>,
    pub parent_primary_image_item_id: Option<String>,
    pub parent_primary_image_tag: Option<String>,
    //pub chapters: Option<Vec<ChapterInfo>>,
    pub location_type: Option<String>,
    pub iso_type: Option<String>,
    #[default("Unknown".to_string())]
    pub media_type: String,
    pub end_date: Option<String>,
    //pub locked_fields: Option<Vec<MetadataFields>>,
    pub trailer_count: Option<i64>,
    pub movie_count: Option<i64>,
    pub series_count: Option<i64>,
    pub program_count: Option<i64>,
    pub episode_count: Option<i64>,
    pub song_count: Option<i64>,
    pub album_count: Option<i64>,
    pub artist_count: Option<i64>,
    pub music_video_count: Option<i64>,
    #[default(true)]
    pub lock_data: bool,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub software: Option<String>,
    pub exposure_time: Option<f64>,
    pub focal_length: Option<f64>,
    pub image_orientation: Option<String>,
    pub aperture: Option<f64>,
    pub shutter_speed: Option<f64>,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
    pub altitude: Option<f64>,
    pub iso_speed_rating: Option<i64>,
    pub series_timer_id: Option<String>,
    pub program_id: Option<String>,
    pub channel_primary_image_tag: Option<String>,
    pub start_date: Option<String>,
    pub completion_percentage: Option<i64>,
    pub is_repeat: Option<bool>,
    pub episode_title: Option<String>,
    pub channel_type: Option<String>,
    pub audio: Option<String>,
    pub is_movie: Option<bool>,
    pub is_sports: Option<bool>,
    pub is_series: Option<bool>,
    pub is_live: Option<bool>,
    pub is_news: Option<bool>,
    pub is_kids: Option<bool>,
    pub is_premiere: Option<bool>,
    pub timer_id: Option<String>,
    // pub current_program: Option<Box<BaseItemDto>>,
    pub has_series_timer: Option<bool>,
    pub has_timer: Option<bool>,
    pub provider_ids: Option<ProviderIds>,
    // internal stuff
    // #[serde(skip)]
    // pub aio_id: Option<String>,
    // #[serde(skip)]
    // pub aio_resource_type: Option<aio::ResourceType>,
    //#[serde(skip)]
    //pub aio_media_type: Option<aio::MediaType>,
    // #[serde(skip)]
    //pub aio_stream: Option<sdks::aio::Stream>,
}

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
)]
#[serde(rename_all = "PascalCase")]
pub enum ItemFilter {
    IsFolder,
    IsNotFolder,
    IsUnplayed,
    IsPlayed,
    IsFavorite,
    IsResumable,
    Likes,
    Dislikes,
    IsFavoriteOrLikes,
}

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
    Default,
)]
#[serde(rename_all = "PascalCase")]
pub enum MediaType {
    AggregateFolder,
    Audio,
    AudioBook,
    BasePluginFolder,
    Book,
    BoxSet,
    Channel,
    ChannelFolderItem,
    CollectionFolder,
    Episode,
    Folder,
    Genre,
    ManualPlaylistsFolder,
    Movie,
    LiveTvChannel,
    LiveTvProgram,
    MusicAlbum,
    MusicArtist,
    MusicGenre,
    MusicVideo,
    Person,
    Photo,
    PhotoAlbum,
    Playlist,
    PlaylistsFolder,
    Program,
    Recording,
    Season,
    Series,
    Studio,
    Trailer,
    TvChannel,
    TvProgram,
    UserRootFolder,
    UserView,
    Video,
    Year,
    #[default]
    Unknown,
}

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
)]
#[serde(rename_all = "PascalCase")]
pub enum SortOrder {
    Ascending,
    Descending,
}

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
)]
#[serde(rename_all = "PascalCase")]
pub enum ItemSortBy {
    Default,
    AiredEpisodeOrder,
    Album,
    AlbumArtist,
    Artist,
    DateCreated,
    OfficialRating,
    DatePlayed,
    PremiereDate,
    StartDate,
    SortName,
    Name,
    Random,
    Runtime,
    CommunityRating,
    ProductionYear,
    PlayCount,
    CriticRating,
    IsFolder,
    IsUnplayed,
    IsPlayed,
    SeriesSortName,
    VideoBitRate,
    AirTime,
    Studio,
    IsFavoriteOrLiked,
    DateLastContentAdded,
    SeriesDatePlayed,
    ParentIndexNumber,
    IndexNumber,
    SimilarityScore,
    SearchScore,
}

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
    //  Default,
)]
#[serde(rename_all = "PascalCase")]
pub enum Status {
    Continuing,
    Ended,
    Unreleased,
    Released,
}

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
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum ItemFields {
    AirTime,
    CanDelete,
    CanDownload,
    ChannelInfo,
    Chapters,
    Trickplay,
    ChildCount,
    CumulativeRunTimeTicks,
    CustomRating,
    DateCreated,
    DateLastMediaAdded,
    DisplayPreferencesId,
    Etag,
    ExternalUrls,
    Genres,
    HomePageUrl,
    ItemCounts,
    MediaSourceCount,
    MediaSources,
    OriginalTitle,
    Overview,
    ParentId,
    Path,
    People,
    PlayAccess,
    ProductionLocations,
    ProviderIds,
    PrimaryImageAspectRatio,
    RecursiveItemCount,
    Settings,
    ScreenshotImageTags,
    SeriesPrimaryImage,
    SeriesStudio,
    SortName,
    SpecialEpisodeNumbers,
    Studios,
    Taglines,
    Tags,
    RemoteTrailers,
    MediaStreams,
    SeasonUserData,
    ServiceName,
    ThemeSongIds,
    ThemeVideoIds,
    ExternalEtag,
    PresentationUniqueKey,
    InheritedParentalRatingValue,
    ExternalSeriesId,
    SeriesPresentationUniqueKey,
    DateLastRefreshed,
    DateLastSaved,
    RefreshState,
    ChannelImage,
    EnableMediaSourceDisplay,
    Width,
    Height,
    ExtraIds,
    LocalTrailerCount,
    #[serde(rename = "IsHD")]
    #[strum(serialize = "IsHD")]
    IsHd,
    SpecialFeatureCount,
}

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
pub enum VideoRange {
    Unknown,
    #[serde(rename = "SDR")]
    Sdr,
    #[serde(rename = "HDR")]
    Hdr,
}

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
pub enum VideoRangeType {
    Unknown,
    #[serde(rename = "SDR")]
    Sdr,
    #[serde(rename = "HDR10")]
    Hdr10,
    #[serde(rename = "HLG")]
    Hlg,
    #[serde(rename = "DOVI")]
    Dovi,
    #[serde(rename = "DOVIWithHDR10")]
    DoviWithHdr10,
    #[serde(rename = "DOVIWithHLG")]
    DoviWithHlg,
    #[serde(rename = "DOVIWithSDR")]
    DoviWithSdr,
    #[serde(rename = "HDR10Plus")]
    Hdr10Plus,
}

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
pub enum CollectionType {
    #[serde(rename = "unknown")]
    Unknown,
    #[serde(rename = "movies")]
    Movies,
    #[serde(rename = "tvshows")]
    Tvshows,
    #[serde(rename = "music")]
    Music,
    #[serde(rename = "musicvideos")]
    Musicvideos,
    #[serde(rename = "trailers")]
    Trailers,
    #[serde(rename = "homevideos")]
    Homevideos,
    #[serde(rename = "boxsets")]
    Boxsets,
    #[serde(rename = "books")]
    Books,
    #[serde(rename = "photos")]
    Photos,
    #[serde(rename = "livetv")]
    Livetv,
    #[serde(rename = "playlists")]
    Playlists,
    #[serde(rename = "folders")]
    Folders,
}
