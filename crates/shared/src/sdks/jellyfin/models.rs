use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde_alias::serde_alias;
use serde_aux::prelude::*;
use serde_with::serde_as;
use serde_with::skip_serializing_none;
use std::collections::HashMap;
use std::str::FromStr;
use uuid::Uuid;

/// Gracefully deserializes `Option<T>` where `T: FromStr`.
/// Returns `None` on missing, null, empty string, or any parse failure.
fn deserialize_optional<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    T: FromStr,
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.and_then(|s| s.parse().ok()))
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult<T> {
    /// Gets or sets the items.
    pub items: Vec<T>,
    /// Gets or sets the total number of records available.
    pub total_record_count: i64,
    /// Gets or sets the index of the first record in Items.
    pub start_index: i32,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BrandingOptions {
    /// Gets or sets the login disclaimer.
    pub login_disclaimer: Option<String>,
    /// Gets or sets the custom CSS.
    pub custom_css: Option<String>,
    /// Gets or sets a value indicating whether to enable the splashscreen.
    pub splashscreen_enabled: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct ServerConfiguration {
    /// Gets or sets a value indicating whether to enable prometheus metrics exporting.
    #[default(Some(false))]
    pub enable_metrics: Option<bool>,
    /// Gets or sets a value indicating whether this instance is port authorized.
    #[default(Some(true))]
    pub is_port_authorized: Option<bool>,
    /// Gets or sets a value indicating whether quick connect is available for use on this server.
    #[default(Some(true))]
    pub quick_connect_available: Option<bool>,
    /// Gets or sets a value indicating whether [enable case-sensitive item ids].
    #[default(Some(true))]
    pub enable_case_sensitive_item_ids: Option<bool>,
    /// Gets or sets the metadata path.
    #[default(Some("/metadata".to_string()))]
    pub metadata_path: Option<String>,
    /// Gets or sets the preferred metadata language.
    #[default(Some("en".to_string()))]
    pub preferred_metadata_language: Option<String>,
    /// Gets or sets the metadata country code.
    #[default(Some("US".to_string()))]
    pub metadata_country_code: Option<String>,
    /// Gets or sets the path to the FFmpeg executable.
    #[default(Some("/usr/bin/ffmpeg".to_string()))]
    pub ffmpeg_path: Option<String>,
    /// Gets or sets the path to the FFprobe executable.
    #[default(Some("/usr/bin/ffprobe".to_string()))]
    pub ffprobe_path: Option<String>,
    /// Gets or sets the cache path.
    #[default(Some("/cache".to_string()))]
    pub cache_path: Option<String>,
    /// Gets or sets the number of days we should retain log files.
    #[default(Some(3))]
    pub log_file_retention_days: Option<i32>,
    /// Gets or sets a value indicating whether this instance is first run.
    #[default(Some(false))]
    pub is_startup_wizard_completed: Option<bool>,
    /// Gets or sets the server name.
    #[default(Some("Remux".to_string()))]
    pub server_name: Option<String>,
    /// Gets or sets the UI language culture.
    #[default(Some("en-US".to_string()))]
    pub ui_language_culture: Option<String>,
    /// Gets or sets a value indicating whether to enable automatic updates.
    #[default(Some(false))]
    pub enable_automatic_updates: Option<bool>,
    /// Gets or sets the path to the transcode temp folder.
    #[default(Some("/transcodes".to_string()))]
    pub transcoding_temp_path: Option<String>,
    /// Remux: AIO service base URL.
    pub aio_url: Option<String>,
    /// Remux: maximum number of items imported per catalog.
    pub catalog_max_items: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StartupConfiguration {
    pub server_name: Option<String>,
    pub preferred_metadata_language: Option<String>,
    pub metadata_country_code: Option<String>,
    #[serde(deserialize_with = "clean_aio_url")]
    pub aio_url: Option<String>,
}

fn clean_aio_url<'de, D>(deserializer: D) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let url: Option<String> = Option::deserialize(deserializer)?;
    match url {
        Some(url) => {
            let cleaned = clean_aio_url_str(&url);
            if cleaned.is_empty() {
                Ok(None)
            } else {
                Ok(Some(cleaned.to_string()))
            }
        }
        None => Ok(None),
    }
}

fn clean_aio_url_str(url: &str) -> &str {
    let url = url.trim_end_matches('/');
    let url = url.strip_suffix("/manifest.json").unwrap_or(url);
    url.strip_suffix("/configure").unwrap_or(url)
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StartupUser {
    pub name: Option<String>,
    pub password: Option<String>,
    pub password_confirm: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LocalizationOption {
    /// Gets or sets the name.
    pub name: String,
    /// Gets or sets the value.
    pub value: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CountryInfo {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_region_name: String,
    pub three_letter_iso_region_name: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CultureDto {
    pub name: String,
    pub display_name: String,
    pub two_letter_iso_language_name: String,
    pub three_letter_iso_language_name: Vec<String>,
}

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

pub type AuthenticateUserByNameResult = AuthenticationResult;

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct PublicSystemInfo {
    pub id: String,
    pub local_address: String,
    pub product_name: String,
    pub server_name: String,
    pub startup_wizard_completed: bool,
    pub version: String,
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
#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
#[serde(rename_all = "PascalCase")]
pub struct VirtualFolderInfo {
    pub name: Option<String>,
    pub locations: Vec<String>,
    pub collection_type: Option<CollectionType>,
    pub library_options: LibraryOptions,
    pub item_id: Option<String>,
    pub primary_image_item_id: Option<String>,
    pub refresh_progress: Option<f64>,
    pub refresh_status: Option<String>,
    /// Remux extension: "manual" or "smart"
    pub collection_kind: Option<String>,
    /// Remux extension: whether this collection is shown in library home
    pub promoted: Option<bool>,
    pub collection_max_items: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateVirtualFolderPayload {
    pub name: String,
    /// Jellyfin collection type: "movies" or "tvshows"
    pub collection_type: Option<String>,
    /// Remux extension: "manual" or "smart"
    pub collection_kind: Option<String>,
    pub promoted: Option<bool>,
}

/// Remux extension: an AIO catalog available for import.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AioCatalogInfo {
    /// Composite AIO identifier: "{kind}:{id}"
    pub aio_id: String,
    pub name: String,
    /// Whether this catalog is enabled for import (promoted=1 on catalog media item)
    pub enabled: Option<bool>,
    /// Per-catalog import item limit
    pub max_items: Option<i64>,
    /// UUID of the catalog media item in the DB (present once the catalog has been enabled)
    pub media_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateVirtualFolderPayload {
    pub id: String,
    pub name: String,
    pub collection_type: Option<String>,
    pub collection_kind: Option<String>,
    pub promoted: Option<bool>,
    pub collection_max_items: Option<i64>,
}

/// Payload for PATCH /items/{id} — partial update, only present fields are written.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PatchItemPayload {
    pub name: Option<String>,
    pub collection_type: Option<String>,
    pub collection_kind: Option<String>,
    /// UUIDs of catalog media items to filter this smart collection by
    pub collection_catalog_filter: Option<Vec<String>>,
    pub promoted: Option<bool>,
}

/// Payload for POST /aio/catalogs/{aio_id} — enable/disable a catalog and set its limit.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateCatalogSettingsPayload {
    pub enabled: bool,
    pub max_items: Option<i64>,
    /// Catalog display name — used when creating the catalog media item for the first time
    pub name: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct FolderStorageInfo {
    /// The path of the folder.
    pub path: Option<String>,
    /// The free space of the underlying storage device.
    pub free_space: Option<i64>,
    /// The used space of the underlying storage device.
    pub used_space: Option<i64>,
    /// The kind of storage device.
    pub storage_type: Option<String>,
    /// The Device Identifier.
    pub device_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryStorageInfo {
    /// The Library Id.
    pub id: Option<String>,
    /// The name of the library.
    pub name: Option<String>,
    /// The storage informations about the folders used in a library.
    pub folders: Option<Vec<FolderStorageInfo>>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SystemStorageInfo {
    /// The program data path.
    pub program_data_folder: Option<FolderStorageInfo>,
    /// The web UI resources path.
    pub web_folder: Option<FolderStorageInfo>,
    /// The items by name path.
    pub image_cache_folder: Option<FolderStorageInfo>,
    /// The cache path.
    pub cache_folder: Option<FolderStorageInfo>,
    /// The log path.
    pub log_folder: Option<FolderStorageInfo>,
    /// The internal metadata path.
    pub internal_metadata_folder: Option<FolderStorageInfo>,
    /// The transcode path.
    pub transcoding_temp_folder: Option<FolderStorageInfo>,
    /// The storage informations of all libraries.
    pub libraries: Option<Vec<LibraryStorageInfo>>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ItemCounts {
    /// The movie count.
    pub movie_count: i32,
    /// The series count.
    pub series_count: i32,
    /// The episode count.
    pub episode_count: i32,
    /// The artist count.
    pub artist_count: i32,
    /// The program count.
    pub program_count: i32,
    /// The trailer count.
    pub trailer_count: i32,
    /// The song count.
    pub song_count: i32,
    /// The album count.
    pub album_count: i32,
    /// The music video count.
    pub music_video_count: i32,
    /// The box set count.
    pub box_set_count: i32,
    /// The book count.
    pub book_count: i32,
    /// The item count.
    pub item_count: i32,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceInfo {
    /// Gets or sets the name.
    pub name: Option<String>,
    /// Gets or sets the custom name.
    pub custom_name: Option<String>,
    /// Gets or sets the access token.
    pub access_token: Option<String>,
    /// Gets or sets the identifier.
    pub id: Option<String>,
    /// Gets or sets the last name of the user.
    pub last_user_name: Option<String>,
    /// Gets or sets the name of the application.
    pub app_name: Option<String>,
    /// Gets or sets the application version.
    pub app_version: Option<String>,
    /// Gets or sets the last user identifier.
    pub last_user_id: Option<Uuid>,
    /// Gets or sets the date last modified.
    pub date_last_activity: Option<DateTime<Utc>>,
    /// Gets or sets the icon URL.
    pub icon_url: Option<String>,
}

// Placeholder for CollectionTypeOptions and LibraryOptions.
// You'll need to define these according to your needs.
#[derive(Debug, Serialize, Deserialize)]
pub enum CollectionTypeOptions {
    // Define your variants here
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryOptions {
    pub enable_photos: Option<bool>,
    pub enable_realtime_monitor: Option<bool>,
    pub enable_chapter_image_extraction: Option<bool>,
    pub extract_chapter_images_during_library_scan: Option<bool>,
    pub prefer_embedded_titles: Option<bool>,
    pub enable_internet_providers: Option<bool>,
    pub enable_automatic_series_grouping: Option<bool>,
    pub save_local_metadata: Option<bool>,
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
    #[serde(deserialize_with = "deserialize_media_types", default)]
    pub exclude_item_types: Option<Vec<MediaType>>,
    #[serde(deserialize_with = "deserialize_media_types", default)]
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
    pub recursive: Option<bool>,
}

impl GetItemsQuery {
    pub fn get_requested_item_types(&self) -> Vec<MediaType> {
        let mut requested: Vec<MediaType> =
            vec![MediaType::Movie, MediaType::Series, MediaType::Episode];

        if let Some(include_types) = &self.include_item_types {
            requested = include_types
                .iter()
                .filter(|t| {
                    matches!(
                        t,
                        MediaType::Movie | MediaType::Series | MediaType::Episode
                    )
                })
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

pub fn deserialize_media_types<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<MediaType>>, D::Error>
where
    D: Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Single(String),
        Multiple(Vec<String>),
    }

    let input = Option::<Input>::deserialize(deserializer)?;

    let types = match input {
        Some(Input::Single(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<MediaType>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(value = %s, "MediaType parse failed, ignoring value");
                    None
                }
            })
            .collect(),
        Some(Input::Multiple(ss)) => ss
            .iter()
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<MediaType>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(value = %s, "MediaType parse failed, ignoring value");
                    None
                }
            })
            .collect(),
        None => return Ok(None),
    };

    Ok(Some(types))
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
    pub video_codec: Option<String>,
    pub video_bit_rate: Option<i64>,
    pub audio_bit_rate: Option<i64>,
    pub audio_channels: Option<i64>,
    pub max_audio_channels: Option<i64>,
    pub audio_sample_rate: Option<i64>,
    pub max_audio_bit_depth: Option<i64>,
    pub width: Option<i64>,
    pub height: Option<i64>,
    pub max_width: Option<i64>,
    pub max_height: Option<i64>,
    pub framerate: Option<f32>,
    pub max_framerate: Option<f32>,
    pub profile: Option<String>,
    pub level: Option<String>,
    pub subtitle_stream_index: Option<i64>,
    pub subtitle_method: Option<String>,
    pub subtitle_codec: Option<String>,
    pub start_time_ticks: Option<i64>,
    pub copy_timestamps: Option<bool>,
    pub transcode_reasons: Option<String>,
    pub require_avc: Option<bool>,
    pub de_interlace: Option<bool>,
    pub max_ref_frames: Option<i64>,
    pub max_video_bit_depth: Option<i64>,
    pub transcoding_max_audio_channels: Option<i64>,
    pub enable_auto_stream_copy: Option<bool>,
    pub allow_video_stream_copy: Option<bool>,
    pub allow_audio_stream_copy: Option<bool>,
    pub break_on_non_key_frames: Option<bool>,
    pub audio_stream_index: Option<i64>,
    pub video_stream_index: Option<i64>,
    pub context: Option<String>,
    pub stream_options: Option<std::collections::HashMap<String, Option<String>>>,
    pub enable_audio_vbr_encoding: Option<bool>,
    pub always_burn_in_subtitle_when_transcoding: Option<bool>,
    pub live_stream_id: Option<String>,
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct TranscodingProfile {
    pub container: Option<String>,
    pub protocol: Option<String>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>, // "Video", "Audio", "Photo"
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct DeviceProfile {
    pub name: Option<String>,
    pub max_streaming_bitrate: Option<i64>,
    pub max_static_bitrate: Option<i64>,
    pub music_streaming_transcoding_bitrate: Option<i64>,
    pub max_static_music_bitrate: Option<i64>,
    pub direct_play_profiles: Vec<DirectPlayProfile>,
    pub transcoding_profiles: Vec<TranscodingProfile>,
    pub container_profiles: Vec<ContainerProfile>,
    pub codec_profiles: Vec<CodecProfile>,
    pub subtitle_profiles: Vec<SubtitleProfile>,
}

impl DeviceProfile {
    /// Returns the first video transcoding profile, if any.
    pub fn video_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        self.transcoding_profiles.iter().find(|p| {
            p.type_
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Video"))
                .unwrap_or(false)
        })
    }

    /// Returns true if the device supports direct play for the given media source.
    pub fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool {
        // Check if any direct play profile matches the media source
        for profile in &self.direct_play_profiles {
            if profile.supports_media_source(media_source) {
                return true;
            }
        }
        false
    }
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct DirectPlayProfile {
    pub container: Option<String>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>, // "Video", "Audio", etc.
}

impl DirectPlayProfile {
    pub fn supports_media_source(&self, media_source: &MediaSourceInfo) -> bool {
        // Check container match
        if let (Some(profile_container), Some(source_container)) =
            (&self.container, &media_source.container)
        {
            if !self.supports_container(source_container) {
                return false;
            }
        }

        // Check video codec match
        if let (Some(profile_video_codec), Some(video_stream)) =
            (&self.video_codec, media_source.video_stream())
        {
            if let Some(video_codec) = &video_stream.codec {
                if !self.supports_video_codec(video_codec) {
                    return false;
                }
            }
        }

        // Check audio codec match
        if let (Some(profile_audio_codec), Some(audio_stream)) =
            (&self.audio_codec, media_source.audio_stream())
        {
            if let Some(audio_codec) = &audio_stream.codec {
                if !self.supports_audio_codec(audio_codec) {
                    return false;
                }
            }
        }

        true
    }

    pub fn supports_container(&self, container: &str) -> bool {
        self.container
            .as_ref()
            .map(|c| c.split(',').any(|c| c.eq_ignore_ascii_case(container)))
            .unwrap_or(true)
    }

    pub fn supports_video_codec(&self, codec: &str) -> bool {
        self.video_codec
            .as_ref()
            .map(|v| v.split(',').any(|v| v.eq_ignore_ascii_case(codec)))
            .unwrap_or(true)
    }

    pub fn supports_audio_codec(&self, codec: &str) -> bool {
        self.audio_codec
            .as_ref()
            .map(|a| a.split(',').any(|a| a.eq_ignore_ascii_case(codec)))
            .unwrap_or(true)
    }
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct ContainerProfile {
    pub type_: Option<String>, // "Video", "Audio", etc.
    pub container: Option<String>,
    pub conditions: Vec<ProfileCondition>,
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct CodecProfile {
    pub type_: Option<String>, // "Video", "Audio", etc.
    pub codec: Option<String>,
    pub conditions: Vec<ProfileCondition>,
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct SubtitleProfile {
    pub format: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub method: Option<SubtitleDeliveryMethod>,
}

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct ProfileCondition {
    pub condition: Option<String>,
    pub property: Option<String>,
    pub value: Option<String>,
    #[serde(rename = "IsRequired")]
    pub is_required: Option<bool>,
}

#[serde_alias(CamelCase, PascalCase)]
#[derive(Default, Debug, Deserialize, Clone)]
#[serde(default)]
#[serde_as]
pub struct PlaybackInfoQuery {
    pub user_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_option_number_from_string")]
    pub max_streaming_bitrate: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_option_number_from_string")]
    pub start_time_ticks: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_option_number_from_string")]
    pub audio_stream_index: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_option_number_from_string")]
    pub subtitle_stream_index: Option<i64>,
    #[serde(default, deserialize_with = "deserialize_option_number_from_string")]
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
    pub device_profile: Option<DeviceProfile>,
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
    pub transcoding_sub_protocol: Option<String>,
    pub transcoding_url: Option<String>,
    // pub type_: Option<MediaSourceType>,
    #[default(false)]
    pub use_most_compatible_transcoding_profile: bool,
    //  pub video3_d_format: Option<Video3DFormat>,
    #[default("VideoFile".to_string())]
    pub video_type: String,
}

impl MediaSourceInfo {
    /// Returns the first video stream, if any.
    pub fn video_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|s| matches!(s.type_, Some(MediaStreamType::Video)))
    }

    /// Returns the first audio stream, if any.
    pub fn audio_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|s| matches!(s.type_, Some(MediaStreamType::Audio)))
    }

    /// Returns the first subtitle stream, if any.
    pub fn subtitle_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|s| matches!(s.type_, Some(MediaStreamType::Subtitle)))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum PlaybackErrorCode {
    NotAllowed,
    NoCompatibleStream,
    RateLimitExceeded,
}

#[skip_serializing_none]
#[derive(Default, Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaybackInfoResponse {
    pub error_code: Option<PlaybackErrorCode>,
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

#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct MetadataEditorInfo {
    pub parental_rating_options: Vec<serde_json::Value>,
    pub countries: Vec<serde_json::Value>,
    pub cultures: Vec<serde_json::Value>,
    pub external_id_infos: Vec<serde_json::Value>,
    pub content_type: Option<String>,
    pub content_type_options: Vec<serde_json::Value>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserConfiguration {
    pub audio_language_preference: Option<String>,
    #[serde(default)]
    pub play_default_audio_track: Option<bool>,
    pub subtitle_language_preference: Option<String>,
    #[serde(default)]
    pub display_missing_episodes: Option<bool>,
    #[serde(default)]
    pub grouped_folders: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub subtitle_mode: Option<SubtitleMode>,
    #[serde(default)]
    pub display_collections_view: Option<bool>,
    #[serde(default)]
    pub enable_local_password: Option<bool>,
    #[serde(default)]
    pub ordered_views: Vec<String>,
    #[serde(default)]
    pub latest_items_excludes: Vec<String>,
    #[serde(default)]
    pub my_media_excludes: Vec<String>,
    #[serde(default)]
    pub hide_played_in_latest: Option<bool>,
    #[serde(default)]
    pub remember_audio_selections: Option<bool>,
    #[serde(default)]
    pub remember_subtitle_selections: Option<bool>,
    #[serde(default)]
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
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub scroll_direction: Option<ScrollDirection>,
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub sort_order: Option<SortOrder>,
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
#[strum(serialize_all = "PascalCase")]
pub enum ScrollDirection {
    Horizontal,
    Vertical,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum PlayMethod {
    Transcode,
    DirectStream,
    DirectPlay,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum RepeatMode {
    RepeatNone,
    RepeatAll,
    RepeatOne,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum SubtitleDeliveryMethod {
    Encode,
    Embed,
    External,
    Hls,
    Drop,
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum SubtitleMode {
    Default,
    Always,
    OnlyForced,
    None,
    Smart,
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
            id: Uuid::new_v4(),
            last_activity_date: None,
            last_login_date: None,
            name: "default".to_string(),
            policy: UserPolicy::default(),
            primary_image_aspect_ratio: None,
            primary_image_tag: None,
            server_id: "remux".to_string(),
            server_name: None,
        }
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
#[strum(serialize_all = "PascalCase")]
pub enum SyncPlayUserAccessType {
    CreateAndJoinGroups,
    JoinGroups,
    None,
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
    #[serde(
        default,
        deserialize_with = "deserialize_optional",
        skip_serializing_if = "Option::is_none"
    )]
    pub sync_play_access: Option<SyncPlayUserAccessType>,
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
            sync_play_access: Some(SyncPlayUserAccessType::CreateAndJoinGroups),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateUserByName {
    pub name: String,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateUserPassword {
    pub current_pw: Option<String>,
    pub new_pw: Option<String>,
    pub reset_password: Option<bool>,
}

#[skip_serializing_none]
#[derive(Default, Deserialize, PartialEq, Serialize, Clone, Debug)]
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

#[derive(
    Default,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RemuxCollectionKind {
    #[default]
    Manual,
    Smart,
}

#[derive(
    Default,
    Clone,
    Debug,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum RemuxMediaKind {
    #[default]
    Movie,
    Series,
    Season,
    Episode,
    Collection,
    Catalog,
    Folder,
    Genre,
    Person,
    Studio,
}

/// Remux-specific fields not part of the Jellyfin spec, nested under `Remux`
/// on BaseItemDto so they're easy to ignore by standard Jellyfin clients.
#[skip_serializing_none]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemuxInfo {
    pub collection_kind: Option<RemuxCollectionKind>,
    pub collection_media_kind: Option<RemuxMediaKind>,
    pub collection_max_items: Option<i64>,
    pub collection_catalog_filter: Option<Vec<Uuid>>,
    pub promoted: Option<bool>,
}

#[skip_serializing_none]
#[derive(default2::Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemDto {
    pub id: Uuid,
    #[default("remux".to_string())]
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
    pub people: Option<Vec<BaseItemPerson>>,
    pub studios: Option<Vec<NameIdPair>>,
    pub genre_items: Option<Vec<NameIdPair>>,
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
    pub collection_kind: Option<String>,
    pub collection_catalog_filter: Option<Vec<String>>,
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
    pub remux: Option<RemuxInfo>,
}

#[skip_serializing_none]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct BaseItemPerson {
    pub id: Uuid,
    pub name: String,
    pub role: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>,
    pub primary_image_tag: Option<String>,
}

#[skip_serializing_none]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NameIdPair {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfoDto {
    //pub play_state: Option<PlayerStateInfo>,
    // pub additional_users: Option<Vec<SessionUserInfo>>,
    //pub capabilities: Option<ClientCapabilitiesDto>,
    pub remote_end_point: Option<String>,
    pub playable_media_types: Vec<MediaType>,
    pub id: Option<String>,
    pub user_id: String,
    pub user_name: Option<String>,
    pub client: Option<String>,
    pub last_activity_date: DateTime<Utc>,
    pub last_playback_check_in: DateTime<Utc>,
    pub last_paused_date: Option<DateTime<Utc>>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub now_playing_item: Option<BaseItemDto>,
    pub now_viewing_item: Option<BaseItemDto>,
    pub device_id: Option<String>,
    pub application_version: Option<String>,
    //  pub transcoding_info: Option<TranscodingInfo>,
    pub is_active: bool,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub now_playing_queue: Option<Vec<QueueItem>>,
    pub now_playing_queue_full_items: Option<Vec<BaseItemDto>>,
    pub has_custom_device_name: bool,
    pub playlist_item_id: Option<String>,
    pub server_id: Option<String>,
    pub user_primary_image_tag: Option<String>,
    // pub supported_commands: Vec<GeneralCommandType>,
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStartInfo {
    pub can_seek: bool,
    pub item_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub media_source_id: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub is_paused: bool,
    pub is_muted: bool,
    pub position_ticks: Option<i64>,
    pub volume_level: Option<i32>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub play_method: Option<PlayMethod>,
    pub live_stream_id: Option<String>,
    pub play_session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub repeat_mode: Option<RepeatMode>,
    pub now_playing_queue: Option<Vec<QueueItem>>,
    pub playlist_item_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackProgressInfo {
    pub can_seek: bool,
    pub item_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub media_source_id: Option<String>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub is_paused: bool,
    pub is_muted: bool,
    pub position_ticks: Option<i64>,
    pub playback_start_time_ticks: Option<i64>,
    pub volume_level: Option<i32>,
    pub brightness: Option<i32>,
    pub aspect_ratio: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub play_method: Option<PlayMethod>,
    pub live_stream_id: Option<String>,
    pub play_session_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_optional")]
    pub repeat_mode: Option<RepeatMode>,
    pub now_playing_queue: Option<Vec<QueueItem>>,
    pub playlist_item_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackStopInfo {
    pub item_id: Option<Uuid>,
    pub session_id: Option<String>,
    pub media_source_id: Option<String>,
    pub position_ticks: Option<i64>,
    pub live_stream_id: Option<String>,
    pub play_session_id: Option<String>,
    pub next_media_type: Option<String>,
    pub playlist_item_id: Option<String>,
    pub now_playing_queue: Option<Vec<QueueItem>>,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueueItem {
    pub id: Uuid,
    #[serde(default)]
    pub playlist_item_id: String,
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

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskInfo {
    /// Gets or sets the name.
    pub name: String,
    /// Gets or sets the state.
    pub state: Option<String>,
    /// Gets or sets the current progress percentage.
    pub current_progress_percentage: Option<f64>,
    /// Gets or sets the id.
    pub id: String,
    /// Gets or sets the last execution result.
    pub last_execution_result: Option<TaskResult>,
    /// Gets or sets the triggers.
    pub triggers: Option<Vec<TaskTriggerInfo>>,
    /// Gets or sets the description.
    pub description: Option<String>,
    /// Gets or sets the category.
    pub category: Option<String>,
    /// Gets or sets a value indicating whether this task is hidden.
    pub is_hidden: Option<bool>,
    /// Gets or sets a value indicating whether this task is enabled.
    pub is_enabled: Option<bool>,
    /// Gets or sets the key.
    pub key: Option<String>,
    /// Gets or sets the last execution date.
    pub last_execution_date: Option<String>,
    /// Gets or sets the can_be_terminated.
    pub can_be_terminated: Option<bool>,
    /// Gets or sets the can_be_deleted.
    pub can_be_deleted: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskResult {
    /// Gets or sets the status.
    pub status: Option<String>,
    /// Gets or sets the name.
    pub name: Option<String>,
    /// Gets or sets the id.
    pub id: Option<String>,
    /// Gets or sets the key.
    pub key: Option<String>,
    /// Gets or sets the error_message.
    pub error_message: Option<String>,
    /// Gets or sets the long_error_message.
    pub long_error_message: Option<String>,
    /// Gets or sets the start_time_utc.
    pub start_time_utc: Option<String>,
    /// Gets or sets the end_time_utc.
    pub end_time_utc: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    /// Gets or sets the type.
    pub r#type: Option<String>,
    /// Gets or sets the time_of_day_ticks.
    pub time_of_day_ticks: Option<i64>,
    /// Gets or sets the interval_ticks.
    pub interval_ticks: Option<i64>,
    /// Gets or sets the day_of_week.
    pub day_of_week: Option<String>,
    /// Gets or sets the max_runtime_ticks.
    pub max_runtime_ticks: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskQueryResult {
    /// Gets or sets the items.
    pub items: Vec<TaskInfo>,
    /// Gets or sets the total number of records available.
    pub total_record_count: i64,
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

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct HlsVideoQuery {
    pub play_session_id: Option<String>,
    pub media_source_id: Option<Uuid>,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub segment_length: Option<i32>,
    pub start_time_ticks: Option<i64>,
    pub max_width: Option<i32>,
    pub max_height: Option<i32>,
    pub video_bit_rate: Option<i32>,
    pub audio_bit_rate: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NetworkConfiguration {
    pub require_https: Option<bool>,
    pub base_url: Option<String>,
    pub public_https_port: Option<i32>,
    pub http_server_port_number: Option<i32>,
    pub https_port_number: Option<i32>,
    pub enable_https: Option<bool>,
    pub is_port_authorized: Option<bool>,
    pub auto_discovery: Option<bool>,
    pub enable_u_pn_p: Option<bool>,
    pub enable_i_pv4: Option<bool>,
    pub enable_i_pv6: Option<bool>,
    pub internal_http_port: Option<i32>,
    pub internal_https_port: Option<i32>,
    pub public_http_port: Option<i32>,
    pub local_network_subnets: Option<Vec<String>>,
    pub local_network_addresses: Option<Vec<String>>,
    pub known_proxies: Option<Vec<String>>,
    pub ignore_virtual_interfaces: Option<bool>,
    pub virtual_interface_names: Option<Vec<String>>,
    pub enable_published_server_uri_by_request: Option<bool>,
    pub published_server_uri_by_subnet: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationInfo {
    pub access_token: Option<String>,
    pub app_name: Option<String>,
    pub date_created: Option<chrono::DateTime<chrono::Utc>>,
    pub is_active: Option<bool>,
}
