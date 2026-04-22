use chrono::{DateTime, Utc};
use nutype::nutype;
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

fn deserialize_optional<'de, D, T>(d: D) -> Result<Option<T>, D::Error>
where
    T: FromStr,
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.and_then(|s| s.parse().ok()))
}

fn deserialize_optional_with_default<'de, D, T>(d: D) -> Result<T, D::Error>
where
    T: FromStr + Default,
    D: serde::Deserializer<'de>,
{
    let s: Option<String> = Option::deserialize(d)?;
    Ok(s.and_then(|s| s.parse().ok()).unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DefaultWebClient {
    #[default]
    Jellyfin,
    Anfiteatro,
}

impl DefaultWebClient {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
            Self::Anfiteatro => "anfiteatro",
        }
    }

    pub fn from_str_lossy(value: &str) -> Self {
        if value.eq_ignore_ascii_case("anfiteatro") {
            Self::Anfiteatro
        } else {
            Self::Jellyfin
        }
    }
}

impl<'de> Deserialize<'de> for DefaultWebClient {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        Ok(Self::from_str_lossy(&value))
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct QueryResult<T> {
    pub items: Vec<T>,
    pub total_record_count: i64,
    pub start_index: i32,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BrandingOptions {
    pub login_disclaimer: Option<String>,
    pub custom_css: Option<String>,
    pub splashscreen_enabled: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct QuickConnectResult {
    pub secret: String,
    pub code: String,
    pub authenticated: bool,
    pub date_added: Option<DateTime<Utc>>,
    pub authentication_token: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateWithQuickConnect {
    #[serde(alias = "secret", default)]
    pub secret: String,
}

/// A sanitized, non-empty URL string used for IPTV channel sources and EPG feeds.
#[nutype(
    sanitize(trim),
    validate(not_empty),
    derive(Debug, Clone, Display, PartialEq, Serialize, Deserialize, AsRef, Deref)
)]
pub struct SourceUrl(String);

/// A validated Jellyfin-compatible username.
///
/// Rules (mirrors Jellyfin's `UserManager.ThrowIfInvalidUsername`):
/// - Leading/trailing whitespace is trimmed.
/// - Must not be empty after trimming.
/// - Maximum 255 characters.
/// - Characters restricted to: word characters (`\w`), spaces, `-`, `'`, `.`, `_`, `@`, `+`.
#[nutype(
    sanitize(trim),
    validate(not_empty, len_char_max = 255, regex = r"^[\w \-'._@+]+$"),
    derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRef, Deref, Display)
)]
pub struct Username(String);

/// A sanitized and validated AIO service URL.
///
/// On construction (including serde deserialization) the value is trimmed and
/// stripped of any trailing `/`, `/manifest.json`, or `/configure` suffix so
/// callers always receive a clean base URL.
#[nutype(
    sanitize(trim, with = |s: String| {
        let s = s.trim_end_matches('/');
        let s = s.strip_suffix("/manifest.json").unwrap_or(s);
        s.strip_suffix("/configure").unwrap_or(s).to_string()
    }),
    validate(not_empty),
    derive(Debug, Clone, PartialEq, Serialize, Deserialize, AsRef, Deref)
)]
pub struct AioUrl(String);

/// Deserialize `Option<AioUrl>`, treating an empty or unsanitizable string as `None`.
fn deserialize_option_aio_url<'de, D>(de: D) -> Result<Option<AioUrl>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw: Option<String> = Option::deserialize(de)?;
    Ok(raw.and_then(|s| AioUrl::try_new(s).ok()))
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct ServerConfiguration {
    #[default(Some(false))]
    pub enable_metrics: Option<bool>,
    #[default(Some(true))]
    pub is_port_authorized: Option<bool>,
    #[default(Some(true))]
    pub quick_connect_available: Option<bool>,
    #[default(Some(true))]
    pub enable_case_sensitive_item_ids: Option<bool>,
    #[default(Some("/metadata".to_string()))]
    pub metadata_path: Option<String>,
    #[default(Some("en".to_string()))]
    pub preferred_metadata_language: Option<String>,
    #[default(Some("US".to_string()))]
    pub metadata_country_code: Option<String>,
    #[default(Some("/usr/bin/ffmpeg".to_string()))]
    pub ffmpeg_path: Option<String>,
    #[default(Some("/usr/bin/ffprobe".to_string()))]
    pub ffprobe_path: Option<String>,
    #[default(Some("/cache".to_string()))]
    pub cache_path: Option<String>,
    #[default(Some(3))]
    pub log_file_retention_days: Option<i32>,
    #[default(Some(false))]
    pub is_startup_wizard_completed: Option<bool>,
    #[default(Some(DefaultWebClient::Jellyfin))]
    pub default_web_client: Option<DefaultWebClient>,
    #[default(Some("Remux".to_string()))]
    pub server_name: Option<String>,
    #[default(Some("en-US".to_string()))]
    pub ui_language_culture: Option<String>,
    #[default(Some(false))]
    pub enable_automatic_updates: Option<bool>,
    #[default(Some("/transcodes".to_string()))]
    pub transcoding_temp_path: Option<String>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_option_aio_url")]
    pub aio_url: Option<AioUrl>,
    pub catalog_max_items: Option<i64>,
    #[default(Some(true))]
    pub p2p_enabled: Option<bool>,
    #[default(Some(0_i64))]
    pub p2p_upload_speed_kbps: Option<i64>,
    #[default(Some(0_i64))]
    pub p2p_download_speed_kbps: Option<i64>,
    #[default(true)]
    pub filter_by_digital_release_date: bool,
    #[default(0_i64)]
    pub digital_release_buffer_days: i64,
    pub tmdb_api_key: Option<String>,
    pub subtitle_languages: Option<Vec<String>>,
    #[default(Some(false))]
    pub enable_subtitles_detail: Option<bool>,
    #[default(Some(true))]
    pub music_enabled: Option<bool>,
    pub jellyfin_url: Option<String>,
    pub jellyfin_api_key: Option<String>,
    #[default(Some(true))]
    pub search_movies_remote: Option<bool>,
    #[default(Some(true))]
    pub search_series_remote: Option<bool>,
    #[default(Some(true))]
    pub search_tracks_remote: Option<bool>,
    #[default(Some(true))]
    pub search_albums_remote: Option<bool>,
    #[default(Some(true))]
    pub search_artists_remote: Option<bool>,
    #[default(Some(true))]
    pub search_people_remote: Option<bool>,
}

// --- Jellyfin import models (used to consume a remote Jellyfin server) ---

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct JellyfinUserPolicy {
    pub is_administrator: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct JellyfinUserDto {
    pub id: Option<String>,
    pub name: Option<String>,
    pub policy: Option<JellyfinUserPolicy>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct JellyfinUserData {
    pub play_count: Option<i64>,
    pub playback_position_ticks: Option<i64>,
    pub last_played_date: Option<DateTime<Utc>>,
    pub played: Option<bool>,
    pub is_favorite: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct JellyfinItem {
    pub id: Option<String>,
    pub name: Option<String>,
    #[serde(rename = "Type")]
    pub item_type: Option<String>,
    pub index_number: Option<i64>,
    pub parent_index_number: Option<i64>,
    pub series_id: Option<String>,
    pub provider_ids: Option<HashMap<String, String>>,
    pub series_provider_ids: Option<HashMap<String, String>>,
    pub user_data: Option<JellyfinUserData>,
}

impl ServerConfiguration {
    const DEFAULT_TMDB_KEY: &'static str = "eyJhbGciOiJIUzI1NiJ9.eyJhdWQiOiIwZDczZTBjYjkxZjM5ZTY3MGIwZWZhNjkxM2FmYmQ1OCIsIm5iZiI6MTUzMjkzOTA3My41MzcsInN1YiI6IjViNWVjYjQxMGUwYTI2MmU5MDA0NjNjMCIsInNjb3BlcyI6WyJhcGlfcmVhZCJdLCJ2ZXJzaW9uIjoxfQ.vfOGe8_35CxhjjZXdnR2iAwdOMIY0VFYMBQrLWuRqn8";

    pub fn get_tmdb_key(&self) -> &str {
        self.tmdb_api_key
            .as_deref()
            .filter(|k| !k.is_empty())
            .unwrap_or(Self::DEFAULT_TMDB_KEY)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StartupConfiguration {
    pub server_name: Option<String>,
    pub preferred_metadata_language: Option<String>,
    pub metadata_country_code: Option<String>,
    pub default_web_client: Option<DefaultWebClient>,
    #[serde(default)]
    #[serde(deserialize_with = "deserialize_option_aio_url")]
    pub aio_url: Option<AioUrl>,
}


#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct StartupUser {
    pub name: Option<Username>,
    pub password: Option<String>,
    pub password_confirm: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LocalizationOption {
    pub name: String,
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
    #[serde(rename = "TwoLetterISOLanguageName")]
    pub two_letter_iso_language_name: String,
    #[serde(rename = "ThreeLetterISOLanguageName")]
    pub three_letter_iso_language_name: String,
    #[serde(rename = "ThreeLetterISOLanguageNames")]
    pub three_letter_iso_language_names: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticateUserByName {
    pub pw: Option<String>,
    pub username: Option<String>,
}

impl<'de> serde::Deserialize<'de> for AuthenticateUserByName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Jellyfin clients send both `Pw` and `Password` in the same request.
        // Using `alias` causes serde to error on duplicate keys, so we
        // deserialize into a flat helper and merge the two fields.
        #[derive(serde::Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct Raw {
            pw: Option<String>,
            password: Option<String>,
            username: Option<String>,
        }
        let raw = Raw::deserialize(d)?;
        Ok(Self {
            pw: raw.pw.or(raw.password),
            username: raw.username,
        })
    }
}

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct AuthenticationResult {
    pub access_token: Option<String>,
    pub server_id: String,
    pub session_info: Option<SessionInfoDto>,
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
#[derive(Debug, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub operating_system_display_name: Option<String>,
    #[default(false)]
    pub has_pending_restart: bool,
    #[default(false)]
    pub is_shutting_down: bool,
    #[default(true)]
    pub supports_library_monitor: bool,
    #[default(8096_u16)]
    pub web_socket_port_number: u16,
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
    pub collection_kind: Option<String>,
    pub promoted: Option<bool>,
    pub collection_max_items: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateVirtualFolderPayload {
    pub name: String,
    pub collection_type: Option<String>,
    pub collection_kind: Option<String>,
    pub promoted: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct AioCatalogInfo {
    pub aio_id: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub max_items: Option<i64>,
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

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct PatchItemPayload {
    pub name: Option<String>,
    pub collection_type: Option<String>,
    pub collection_kind: Option<String>,
    pub smart_filter: Option<CollectionFilter>,
    pub promoted: Option<bool>,
    pub tags: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateCatalogSettingsPayload {
    pub enabled: bool,
    pub max_items: Option<i64>,
    pub name: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct FolderStorageInfo {
    pub path: Option<String>,
    pub free_space: Option<i64>,
    pub used_space: Option<i64>,
    pub storage_type: Option<String>,
    pub device_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LibraryStorageInfo {
    pub id: Option<String>,
    pub name: Option<String>,
    pub folders: Option<Vec<FolderStorageInfo>>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct SystemStorageInfo {
    pub program_data_folder: Option<FolderStorageInfo>,
    pub web_folder: Option<FolderStorageInfo>,
    pub image_cache_folder: Option<FolderStorageInfo>,
    pub cache_folder: Option<FolderStorageInfo>,
    pub log_folder: Option<FolderStorageInfo>,
    pub internal_metadata_folder: Option<FolderStorageInfo>,
    pub transcoding_temp_folder: Option<FolderStorageInfo>,
    pub libraries: Option<Vec<LibraryStorageInfo>>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct ItemCounts {
    pub movie_count: i32,
    pub series_count: i32,
    pub episode_count: i32,
    pub artist_count: i32,
    pub program_count: i32,
    pub trailer_count: i32,
    pub song_count: i32,
    pub album_count: i32,
    pub music_video_count: i32,
    pub box_set_count: i32,
    pub book_count: i32,
    pub item_count: i32,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub custom_name: Option<String>,
    pub access_token: Option<String>,
    pub id: Option<String>,
    pub last_user_name: Option<String>,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
    pub last_user_id: Option<Uuid>,
    pub date_last_activity: Option<DateTime<Utc>>,
    pub icon_url: Option<String>,
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
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub has_theme_song: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub has_theme_video: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub has_subtitles: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub has_special_feature: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
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
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
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
    #[serde(deserialize_with = "deserialize_sort_by", default)]
    pub sort_by: Option<Vec<ItemSortBy>>,
    #[serde(deserialize_with = "deserialize_sort_order", default)]
    pub sort_order: Option<Vec<SortOrder>>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub enable_images: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub enable_user_data: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub enable_total_record_count: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub enable_resumable: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub enable_rewatching: Option<bool>,
    #[serde(default, deserialize_with = "deserialize_option_bool_from_anything")]
    pub disable_first_episode: Option<bool>,
    pub next_up_date_cutoff: Option<String>,
    pub years: Option<Vec<i64>>,
    pub genres: Option<Vec<String>>,
    pub genre_ids: Option<Vec<String>>,
    pub official_ratings: Option<Vec<String>>,
    pub tags: Option<Vec<String>>,
    #[serde(deserialize_with = "deserialize_media_types", default)]
    pub media_types: Option<Vec<MediaType>>,
    pub filters: Option<Vec<ItemFilter>>,
    pub person_ids: Option<Vec<String>>,
    pub person_types: Option<Vec<String>>,
    pub studios: Option<Vec<String>>,
    pub studio_ids: Option<Vec<String>>,
    pub exclude_artist_ids: Option<Vec<String>>,
    #[serde(default, deserialize_with = "deserialize_uuids")]
    pub artist_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_uuids")]
    pub contributing_artist_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_uuids")]
    pub album_artist_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_uuids")]
    pub album_ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_uuids")]
    pub ids: Option<Vec<Uuid>>,
    #[serde(default, deserialize_with = "deserialize_bool_from_anything")]
    pub recursive: bool,
    pub series_id: Option<Uuid>,
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
                        MediaType::Movie
                            | MediaType::Series
                            | MediaType::Episode
                            | MediaType::TvChannel
                            | MediaType::LiveTvChannel
                            | MediaType::TvProgram
                            | MediaType::LiveTvProgram
                            | MediaType::Program
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

fn bool_true() -> bool {
    true
}

fn deserialize_option_bool_from_anything<'de, D>(d: D) -> Result<Option<bool>, D::Error>
where
    D: Deserializer<'de>,
{
    let opt = Option::<serde_json::Value>::deserialize(d)?;
    match opt {
        None => Ok(None),
        Some(v) => deserialize_bool_from_anything(v)
            .map(Some)
            .map_err(serde::de::Error::custom),
    }
}

/// Generic helper: deserializes an optional comma-separated (or repeated) query-param
/// value into `Option<Vec<T>>` for any `T: FromStr`.
fn deserialize_comma_str<'de, D, T>(deserializer: D) -> Result<Option<Vec<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: std::fmt::Display,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Input {
        Single(String),
        Multiple(Vec<String>),
    }

    let input = Option::<Input>::deserialize(deserializer)?;
    let type_name = std::any::type_name::<T>();

    let values = match input {
        Some(Input::Single(s)) => s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<T>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(value = %s, error = %e, type_name, "parse failed, ignoring value");
                    None
                }
            })
            .collect(),
        Some(Input::Multiple(ss)) => ss
            .iter()
            .flat_map(|s| s.split(','))
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .filter_map(|s| match s.parse::<T>() {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!(value = %s, error = %e, type_name, "parse failed, ignoring value");
                    None
                }
            })
            .collect(),
        None => return Ok(None),
    };

    Ok(Some(values))
}

pub fn deserialize_fields<'de, D>(d: D) -> Result<Option<Vec<ItemFields>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_comma_str(d)
}

pub fn deserialize_media_types<'de, D>(d: D) -> Result<Option<Vec<MediaType>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_comma_str(d)
}

pub fn deserialize_sort_by<'de, D>(d: D) -> Result<Option<Vec<ItemSortBy>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_comma_str(d)
}

pub fn deserialize_sort_order<'de, D>(d: D) -> Result<Option<Vec<SortOrder>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_comma_str(d)
}

pub fn deserialize_uuids<'de, D>(d: D) -> Result<Option<Vec<Uuid>>, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_comma_str(d)
}

#[derive(Default, Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct VideoStreamQuery {
    pub container: Option<String>,
    #[serde(alias = "static", alias = "Static")]
    pub static_: Option<bool>,
    pub params: Option<String>,
    pub tag: Option<String>,
    pub device_profile_id: Option<String>,
    pub play_session_id: Option<String>,
    pub segment_container: Option<String>,
    pub segment_length: Option<i64>,
    pub min_segments: Option<i64>,
    #[serde(alias = "mediaSourceId")]
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
    pub fn video_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        self.transcoding_profiles.iter().find(|p| {
            p.type_
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Video"))
                .unwrap_or(false)
        })
    }

    pub fn audio_transcoding_profile(&self) -> Option<&TranscodingProfile> {
        self.transcoding_profiles.iter().find(|p| {
            p.type_
                .as_deref()
                .map(|t| t.eq_ignore_ascii_case("Audio"))
                .unwrap_or(false)
        })
    }

    pub fn subtitle_delivery_method(&self, codec: &str) -> Option<SubtitleDeliveryMethod> {
        self.subtitle_profiles
            .iter()
            .find(|p| {
                p.format
                    .as_deref()
                    .map(|f| f.eq_ignore_ascii_case(codec))
                    .unwrap_or(false)
            })
            .and_then(|p| p.method.clone())
    }

    pub fn supports_direct_play(&self, media_source: &MediaSourceInfo) -> bool {
        self.check_direct_play(media_source).is_empty()
    }

    pub fn check_direct_play(
        &self,
        media_source: &MediaSourceInfo,
    ) -> TranscodeReasons {
        let source_has_video = media_source.video_stream().is_some();
        let mut best: Option<TranscodeReasons> = None;
        for profile in &self.direct_play_profiles {
            // Skip profiles whose type is explicitly incompatible with the source.
            if let Some(t) = &profile.type_ {
                if t.eq_ignore_ascii_case("Video") && !source_has_video { continue; }
                if t.eq_ignore_ascii_case("Audio") && source_has_video { continue; }
            }
            let reasons = profile.check_reasons(media_source);
            if reasons.is_empty() {
                return reasons; // perfect match
            }
            best = Some(match best {
                None => reasons,
                Some(prev) => {
                    if reasons.0.len() < prev.0.len() { reasons } else { prev }
                }
            });
        }
        best.unwrap_or_else(|| {
            let mut r = TranscodeReasons::default();
            r.insert(TranscodeReason::ContainerNotSupported("no matching profile".into()));
            r
        })
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
        self.check_reasons(media_source).is_empty()
    }

    pub fn check_reasons(&self, media_source: &MediaSourceInfo) -> TranscodeReasons {
        let mut reasons = TranscodeReasons::default();

        // Check container match
        match (&self.container, &media_source.container) {
            (Some(profile_container), None) => {
                reasons.insert(TranscodeReason::ContainerNotSupported(
                    format!("profile={profile_container} source=(none)"),
                ));
            }
            (Some(profile_container), Some(source_container)) => {
                if !self.supports_container(source_container) {
                    reasons.insert(TranscodeReason::ContainerNotSupported(
                        format!("profile={profile_container} source={source_container}"),
                    ));
                }
            }
            _ => {}
        }

        // Check video codec match
        if let (Some(profile_codec), Some(video_stream)) =
            (&self.video_codec, media_source.video_stream())
        {
            if let Some(video_codec) = &video_stream.codec {
                if !self.supports_video_codec(video_codec) {
                    reasons.insert(TranscodeReason::VideoCodecNotSupported(
                        format!("profile={profile_codec} source={video_codec}"),
                    ));
                }
            }
        }

        // Check audio codec match
        if let (Some(profile_codec), Some(audio_stream)) =
            (&self.audio_codec, media_source.audio_stream())
        {
            if let Some(audio_codec) = &audio_stream.codec {
                if !self.supports_audio_codec(audio_codec) {
                    reasons.insert(TranscodeReason::AudioCodecNotSupported(
                        format!("profile={profile_codec} source={audio_codec}"),
                    ));
                }
            }
        }

        reasons
    }

    pub fn supports_container(&self, container: &str) -> bool {
        // Normalize aliases: mp4 and m4a are the same format.
        let aliases: &[&str] = match container.to_ascii_lowercase().as_str() {
            "mp4" => &["mp4", "m4a"],
            "m4a" => &["m4a", "mp4"],
            _ => &[],
        };
        self.container
            .as_ref()
            .map(|c| {
                c == "*"
                    || c.split(',').any(|c| {
                        let c = c.trim();
                        c.eq_ignore_ascii_case(container)
                            || aliases.iter().any(|a| c.eq_ignore_ascii_case(a))
                    })
            })
            .unwrap_or(true)
    }

    pub fn supports_video_codec(&self, codec: &str) -> bool {
        self.video_codec
            .as_ref()
            .map(|v| {
                v == "*" || v.split(',').any(|v| v.trim().eq_ignore_ascii_case(codec))
            })
            .unwrap_or(true)
    }

    pub fn supports_audio_codec(&self, codec: &str) -> bool {
        self.audio_codec
            .as_ref()
            .map(|a| {
                a == "*" || a.split(',').any(|a| a.trim().eq_ignore_ascii_case(codec))
            })
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
    #[serde(default)] // Always serialize, even if empty
    pub items: Vec<BaseItemDto>,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub start_index: u32,

    // #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_record_count: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum TranscodeReason {
    ContainerNotSupported(String),
    VideoCodecNotSupported(String),
    AudioCodecNotSupported(String),
    SubtitleCodecNotSupported(String),
    ContainerBitrateExceedsLimit,
}

impl TranscodeReason {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ContainerNotSupported(_) => "ContainerNotSupported",
            Self::VideoCodecNotSupported(_) => "VideoCodecNotSupported",
            Self::AudioCodecNotSupported(_) => "AudioCodecNotSupported",
            Self::SubtitleCodecNotSupported(_) => "SubtitleCodecNotSupported",
            Self::ContainerBitrateExceedsLimit => "ContainerBitrateExceedsLimit",
        }
    }
}

impl std::fmt::Debug for TranscodeReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ContainerNotSupported(d) => write!(f, "ContainerNotSupported({d})"),
            Self::VideoCodecNotSupported(d) => write!(f, "VideoCodecNotSupported({d})"),
            Self::AudioCodecNotSupported(d) => write!(f, "AudioCodecNotSupported({d})"),
            Self::SubtitleCodecNotSupported(d) => write!(f, "SubtitleCodecNotSupported({d})"),
            Self::ContainerBitrateExceedsLimit => write!(f, "ContainerBitrateExceedsLimit"),
        }
    }
}

#[derive(Default, Clone, PartialEq, Eq)]
pub struct TranscodeReasons(pub Vec<TranscodeReason>);

impl<'de> serde::Deserialize<'de> for TranscodeReasons {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names = Vec::<String>::deserialize(d)?;
        Ok(Self::from_query_value(&names.join(",")))
    }
}

impl std::fmt::Debug for TranscodeReasons {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl serde::Serialize for TranscodeReasons {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        let mut seq = s.serialize_seq(Some(self.0.len()))?;
        for r in &self.0 {
            seq.serialize_element(r.name())?;
        }
        seq.end()
    }
}

impl TranscodeReasons {
    pub fn insert(&mut self, reason: TranscodeReason) {
        if !self.contains(&reason) {
            self.0.push(reason);
        }
    }

    pub fn contains(&self, reason: &TranscodeReason) -> bool {
        let d = std::mem::discriminant(reason);
        self.0.iter().any(|r| std::mem::discriminant(r) == d)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn to_query_value(&self) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        Some(self.0.iter().map(|r| r.name()).collect::<Vec<_>>().join(","))
    }

    pub fn from_query_value(s: &str) -> Self {
        let mut out = Self::default();
        for part in s.split(',') {
            let reason = match part.trim() {
                "ContainerNotSupported" => Some(TranscodeReason::ContainerNotSupported(String::new())),
                "VideoCodecNotSupported" => Some(TranscodeReason::VideoCodecNotSupported(String::new())),
                "AudioCodecNotSupported" => Some(TranscodeReason::AudioCodecNotSupported(String::new())),
                "SubtitleCodecNotSupported" => Some(TranscodeReason::SubtitleCodecNotSupported(String::new())),
                "ContainerBitrateExceedsLimit" => Some(TranscodeReason::ContainerBitrateExceedsLimit),
                _ => None,
            };
            if let Some(r) = reason {
                out.insert(r);
            }
        }
        out
    }
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TranscodingInfo {
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    pub container: Option<String>,
    pub is_video_direct: bool,
    pub is_audio_direct: bool,
    pub bitrate: Option<i64>,
    pub framerate: Option<f32>,
    pub completion_percentage: Option<f64>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub audio_channels: Option<i32>,
    pub hardware_acceleration_type: Option<String>,
    pub transcode_reasons: TranscodeReasons,
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
    pub e_tag: Uuid,
    pub encoder_path: Option<String>,
    //  pub encoder_protocol: Option<MediaProtocol>,
    pub fallback_max_streaming_bitrate: Option<i64>,
    pub formats: Option<Vec<String>>,
    #[default(false)]
    pub gen_pts_input: bool,
    #[default(false)]
    pub has_segments: bool,
    pub id: Uuid,
    #[default(false)]
    pub ignore_dts: bool,
    #[default(false)]
    pub ignore_index: bool,
    #[default(false)]
    pub is_infinite_stream: bool,
    #[default(false)]
    pub is_remote: bool,
    //pub iso_type: Option<IsoType>,
    pub live_stream_id: Option<String>,
    //pub media_attachments: Option<Vec<MediaAttachment>>,
    #[default(vec![])]
    pub media_streams: Vec<MediaStream>,
    pub name: Option<String>,
    pub open_token: Option<String>,
    pub path: Option<String>,
    #[default("File".to_string())]
    pub protocol: String,
    #[default(false)]
    pub read_at_native_framerate: bool,
    //pub required_http_headers: Option<HashMap<String, Option<String>>>,
    #[default(false)]
    pub requires_closing: bool,
    #[default(false)]
    pub requires_looping: bool,
    #[default(false)]
    pub requires_opening: bool,
    pub run_time_ticks: Option<i64>,
    pub size: Option<i64>,
    #[default(true)]
    pub supports_direct_play: bool,
    #[default(true)]
    pub supports_direct_stream: bool,
    pub supports_external_stream: Option<bool>,
    #[default(true)]
    pub supports_probing: bool,
    #[default(false)]
    pub supports_transcoding: bool,
    //  pub timestamp: Option<TransportStreamTimestamp>,
    pub transcoding_container: Option<String>,
    #[default("http".to_string())]
    pub transcoding_sub_protocol: String,
    pub transcoding_url: Option<String>,
    #[default("Default".to_string())]
    pub type_: String,
    #[default(false)]
    pub use_most_compatible_transcoding_profile: bool,
    //  pub video3_d_format: Option<Video3DFormat>,
    #[default("VideoFile".to_string())]
    pub video_type: String,
}

impl MediaSourceInfo {
    pub fn video_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|s| matches!(s.type_, Some(MediaStreamType::Video)))
    }

    pub fn audio_stream(&self) -> Option<&MediaStream> {
        self.media_streams
            .iter()
            .find(|s| matches!(s.type_, Some(MediaStreamType::Audio)))
    }

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
#[derive(Debug, Clone, default2::Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserConfiguration {
    pub audio_language_preference: Option<String>,
    #[default(true)]
    pub play_default_audio_track: bool,
    pub subtitle_language_preference: Option<String>,
    pub display_missing_episodes: bool,
    #[serde(default)]
    pub grouped_folders: Vec<String>,
    #[default(SubtitleMode::Default)]
    #[serde(default, deserialize_with = "deserialize_optional_with_default")]
    pub subtitle_mode: SubtitleMode,
    pub display_collections_view: bool,
    pub enable_local_password: bool,
    #[serde(default)]
    pub ordered_views: Vec<String>,
    #[serde(default)]
    pub latest_items_excludes: Vec<String>,
    #[serde(default)]
    pub my_media_excludes: Vec<String>,
    #[default(true)]
    pub hide_played_in_latest: bool,
    #[default(true)]
    pub remember_audio_selections: bool,
    #[default(true)]
    pub remember_subtitle_selections: bool,
    #[default(true)]
    pub enable_next_episode_auto_play: bool,
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
    #[serde(default)]
    pub custom_prefs: HashMap<String, Option<String>>,
    #[serde(default, deserialize_with = "deserialize_optional_with_default")]
    pub scroll_direction: ScrollDirection,
    pub show_backdrop: bool,
    pub remember_sorting: bool,
    #[serde(default, deserialize_with = "deserialize_optional_with_default")]
    pub sort_order: SortOrder,
    pub show_sidebar: bool,
    pub client: Option<String>,
}

#[derive(
    Default,
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
    #[default]
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
    Default,
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
    #[default]
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
    LogoImageAspectRatio,
    Thumb,
}

#[skip_serializing_none]
#[derive(Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct UserDto {
    pub configuration: Option<UserConfiguration>,
    pub enable_auto_login: Option<bool>,
    pub has_configured_easy_password: bool,
    pub has_configured_password: bool,
    pub has_password: bool,
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
            has_configured_easy_password: false,
            has_configured_password: true,
            has_password: true,
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
    Default,
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
    #[default]
    CreateAndJoinGroups,
    JoinGroups,
    None,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Clone, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct UserPolicy {
    pub is_administrator: bool,
    #[default(true)]
    pub is_hidden: bool,
    #[default(false)]
    pub enable_collection_management: bool,
    #[default(false)]
    pub enable_subtitle_management: bool,
    #[default(false)]
    pub enable_lyric_management: bool,
    #[default(false)]
    pub is_disabled: bool,
    pub blocked_tags: Option<Vec<String>>,
    pub allowed_tags: Option<Vec<String>>,
    #[default(true)]
    pub enable_user_preference_access: bool,
    pub access_schedules: Option<Vec<String>>,
    pub block_unrated_items: Option<Vec<String>>,
    pub enable_remote_control_of_other_users: bool,
    #[default(true)]
    pub enable_shared_device_control: bool,
    #[default(true)]
    pub enable_remote_access: bool,
    #[default(true)]
    pub enable_live_tv_management: bool,
    #[default(true)]
    pub enable_live_tv_access: bool,
    #[default(true)]
    pub enable_media_playback: bool,
    #[default(true)]
    pub enable_audio_playback_transcoding: bool,
    #[default(true)]
    pub enable_video_playback_transcoding: bool,
    #[default(true)]
    pub enable_playback_remuxing: bool,
    pub force_remote_source_transcoding: bool,
    pub enable_content_deletion: bool,
    pub enable_content_deletion_from_folders: Option<Vec<String>>,
    #[default(true)]
    pub enable_content_downloading: bool,
    #[default(true)]
    pub enable_sync_transcoding: bool,
    #[default(true)]
    pub enable_media_conversion: bool,
    pub enabled_devices: Option<Vec<String>>,
    #[default(true)]
    pub enable_all_devices: bool,
    pub enabled_channels: Option<Vec<String>>,
    #[default(true)]
    pub enable_all_channels: bool,
    pub enabled_folders: Option<Vec<String>>,
    #[default(true)]
    pub enable_all_folders: bool,
    pub invalid_login_attempt_count: i64,
    #[default(-1)]
    pub login_attempts_before_lockout: i64,
    pub max_active_sessions: i64,
    #[default(true)]
    pub enable_public_sharing: bool,
    pub blocked_media_folders: Option<Vec<String>>,
    pub blocked_channels: Option<Vec<String>>,
    pub remote_client_bitrate_limit: i64,
    #[default("Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider".to_string())]
    #[serde(default = "default_authentication_provider_id")]
    pub authentication_provider_id: String,
    #[default("Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider".to_string())]
    #[serde(default = "default_password_reset_provider_id")]
    pub password_reset_provider_id: String,
    #[serde(default, deserialize_with = "deserialize_optional_with_default")]
    #[default(SyncPlayUserAccessType::CreateAndJoinGroups)]
    pub sync_play_access: SyncPlayUserAccessType,
}

fn default_authentication_provider_id() -> String {
    "Jellyfin.Server.Implementations.Users.DefaultAuthenticationProvider".to_string()
}

fn default_password_reset_provider_id() -> String {
    "Jellyfin.Server.Implementations.Users.DefaultPasswordResetProvider".to_string()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreateUserByName {
    pub name: Username,
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
    pub audio_spatial_format: Option<String>,
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
    pub delivery_method: Option<SubtitleDeliveryMethod>,
    pub delivery_url: Option<String>,
    pub display_title: Option<String>,
    pub dv_bl_signal_compatibility_id: Option<i64>,
    pub dv_level: Option<i64>,
    pub dv_profile: Option<i64>,
    pub dv_version_major: Option<i64>,
    pub dv_version_minor: Option<i64>,
    pub el_present_flag: Option<i64>,
    pub height: Option<i64>,
    #[serde(default)]
    pub index: i64,
    pub is_anamorphic: Option<bool>,
    pub is_avc: Option<bool>,
    pub is_default: Option<bool>,
    #[serde(default)]
    pub is_external: bool,
    pub is_external_url: Option<bool>,
    #[serde(default)]
    pub is_forced: bool,
    #[serde(default)]
    pub is_hearing_impaired: bool,
    #[serde(default)]
    pub is_interlaced: bool,
    #[serde(default)]
    pub is_text_subtitle_stream: bool,
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
    pub supports_external_stream: bool,
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
    pub tvdb: Option<String>,
    pub aio: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AnfiteatroReleaseStatus {
    pub local_commit: Option<String>,
    pub local_version_display: Option<String>,
    pub latest_version_tag: Option<String>,
    pub latest_commit: Option<String>,
    pub latest_release_url: Option<String>,
    pub update_available: bool,
    pub check_error: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct AnfiteatroInstallResult {
    pub installed_tag: Option<String>,
    pub installed_commit: Option<String>,
    pub local_version_display: Option<String>,
    pub changed: bool,
    pub message: String,
}

#[skip_serializing_none]
#[derive(default2::Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UserItemDataDto {
    pub rating: Option<f32>,
    #[default(false)]
    pub played: bool,
    pub last_played_date: Option<DateTime<Utc>>,
    #[default(0_i64)]
    pub playback_position_ticks: i64,
    #[default(0_i32)]
    pub play_count: i32,
    #[default(false)]
    pub is_favorite: bool,
    pub likes: Option<bool>,
    pub last_liked_date: Option<DateTime<Utc>>,
    pub favorite_added_date: Option<DateTime<Utc>>,
    pub played_percentage: Option<f32>,
    pub last_updated: Option<DateTime<Utc>>,
    pub unplayed_item_count: Option<i64>,
    #[default(String::new())]
    pub key: String,
    pub item_id: Uuid,
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

/// Operators for numeric fields (Year, Rating).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NumericOp {
    Eq,
    NotEq,
    Gt,
    Lt,
}

/// Operators for text/set fields (Genre, Tag, Studio, etc.).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SetOp {
    Is,
    IsNot,
    In,
    NotIn,
}

/// One condition in a smart collection filter.
/// Each variant carries its own typed value(s) and only the operators valid for that field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum FilterRule {
    Genre          { op: SetOp,     values: Vec<String> },
    Year           { op: NumericOp, value: i64 },
    RatingAudience { op: NumericOp, value: f64 },
    RatingCritic   { op: NumericOp, value: f64 },
    Certification  { op: SetOp,     values: Vec<String> },
    Tag            { op: SetOp,     values: Vec<String> },
    Studio         { op: SetOp,     values: Vec<String> },
    Catalog        { op: SetOp,     values: Vec<String> },
    HasTrailer     { value: bool },
    Country        { op: SetOp,     values: Vec<String> },
    Person         { op: SetOp,     values: Vec<String> },
}

/// Whether all rules must match (AND) or any rule must match (OR).
#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilterMatchMode {
    #[default]
    All,
    Any,
}

/// The filter config stored on a smart collection.
/// Deserialised directly into `MediaFilter.filter_rules` / `filter_match` at query time.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CollectionFilter {
    #[serde(default)]
    pub match_mode: FilterMatchMode,
    #[serde(default)]
    pub rules: Vec<FilterRule>,
}

#[skip_serializing_none]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemuxInfo {
    pub collection_kind: Option<RemuxCollectionKind>,
    pub collection_media_kind: Option<RemuxMediaKind>,
    pub collection_max_items: Option<i64>,
    pub smart_filter: Option<CollectionFilter>,
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
    pub has_lyrics: Option<bool>,
    pub preferred_metadata_language: Option<String>,
    pub preferred_metadata_country_code: Option<String>,
    pub supports_sync: Option<bool>,
    pub container: Option<String>,
    pub sort_name: Option<String>,
    pub forced_sort_name: Option<String>,
    pub video_3d_format: Option<String>,
    //#[serde_as(as = "Option<DisplayFromStr>")]
    pub premiere_date: Option<DateTime<Utc>>,
    // Remux: digital/home release date, distinct from theatrical premiere_date
    pub digital_release_date: Option<DateTime<Utc>>,
    pub external_urls: Option<Vec<ExternalUrl>>,
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
    pub remote_trailers: Option<Vec<ExternalUrl>>,
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
    pub artists: Option<Vec<String>>,
    pub artist_items: Option<Vec<NameIdPair>>,
    pub album: Option<String>,
    pub collection_type: Option<CollectionType>,
    pub collection_kind: Option<String>,
    pub collection_catalog_filter: Option<Vec<String>>,
    pub display_order: Option<String>,
    pub album_id: Option<String>,
    pub album_primary_image_tag: Option<String>,
    pub series_primary_image_tag: Option<String>,
    pub album_artist: Option<String>,
    pub album_artists: Option<Vec<NameIdPair>>,
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
    #[default(MediaType::Unknown)]
    pub media_type: MediaType,
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

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlayerStateInfo {
    pub position_ticks: Option<i64>,
    pub can_seek: bool,
    pub is_paused: bool,
    pub is_muted: bool,
    pub volume_level: Option<i32>,
    pub audio_stream_index: Option<i32>,
    pub subtitle_stream_index: Option<i32>,
    pub media_source_id: Option<String>,
    pub play_method: Option<String>,
    pub repeat_mode: String,
    pub playback_order: String,
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ClientCapabilitiesDto {
    pub playable_media_types: Vec<String>,
    pub supported_commands: Vec<String>,
    pub supports_media_control: bool,
    pub supports_persistent_identifier: bool,
}

#[derive(Debug, Clone, default2::Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SessionInfoDto {
    pub play_state: Option<PlayerStateInfo>,
    pub additional_users: Vec<serde_json::Value>,
    pub capabilities: Option<ClientCapabilitiesDto>,
    pub remote_end_point: Option<String>,
    pub playable_media_types: Vec<String>,
    pub id: Option<String>,
    #[default(String::new())]
    pub user_id: String,
    pub user_name: Option<String>,
    pub client: Option<String>,
    #[default(Utc::now())]
    pub last_activity_date: DateTime<Utc>,
    #[default(Utc::now())]
    pub last_playback_check_in: DateTime<Utc>,
    pub last_paused_date: Option<DateTime<Utc>>,
    pub device_name: Option<String>,
    pub device_type: Option<String>,
    pub now_playing_item: Option<BaseItemDto>,
    pub now_viewing_item: Option<BaseItemDto>,
    pub device_id: Option<String>,
    pub application_version: Option<String>,
    pub transcoding_info: Option<TranscodingInfo>,
    pub is_active: bool,
    pub supports_media_control: bool,
    pub supports_remote_control: bool,
    pub now_playing_queue: Option<Vec<QueueItem>>,
    pub now_playing_queue_full_items: Option<Vec<BaseItemDto>>,
    pub has_custom_device_name: bool,
    pub playlist_item_id: Option<String>,
    pub server_id: String,
    pub user_primary_image_tag: Option<String>,
    pub supported_commands: Vec<String>,
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
#[strum(ascii_case_insensitive)]
#[serde(rename_all = "PascalCase")]
pub enum ItemFilter {
    IsFolder,
    IsNotFolder,
    #[serde(alias = "IsUnPlayed")]
    IsUnplayed,
    #[serde(alias = "IsPlayed")]
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
#[strum(ascii_case_insensitive)]
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
    Default,
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
#[strum(ascii_case_insensitive)]
#[serde(rename_all = "PascalCase")]
pub enum SortOrder {
    #[default]
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
#[strum(ascii_case_insensitive)]
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
#[strum(ascii_case_insensitive, serialize_all = "PascalCase")]
#[serde(rename_all = "PascalCase")]
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
    SeriesId,
    SeasonId,
    SeasonName,
    CollectionType,
    LogoImageAspectRatio,
    PremiereDate,
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
    UserData,
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
    OfficialRating,
    CommunityRating,
    CriticRating,
    RunTimeTicks,
    ProductionYear,
    ImageTags,
    BackdropImageTags,
    BasicSyncInfo,
    SeriesName,
    ParentIndexNumber,
    IndexNumber,
    Status,
    ParentBackdropItemId,
    ParentBackdropImageTags,
    Id,
    Name,
    Artists,
    AlbumId,
    ArtistItems,
    PrimaryImageTag,
    #[serde(other)]
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
    pub name: String,
    pub state: Option<String>,
    pub current_progress_percentage: Option<f64>,
    pub id: String,
    pub last_execution_result: Option<TaskResult>,
    pub triggers: Option<Vec<TaskTriggerInfo>>,
    pub description: Option<String>,
    pub category: Option<String>,
    pub is_hidden: Option<bool>,
    pub is_enabled: Option<bool>,
    pub key: Option<String>,
    pub last_execution_date: Option<String>,
    pub can_be_terminated: Option<bool>,
    pub can_be_deleted: Option<bool>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskResult {
    pub status: Option<String>,
    pub name: Option<String>,
    pub id: Option<String>,
    pub key: Option<String>,
    pub error_message: Option<String>,
    pub long_error_message: Option<String>,
    pub start_time_utc: Option<String>,
    pub end_time_utc: Option<String>,
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumIter,
    strum_macros::EnumString,
)]
#[strum(serialize_all = "PascalCase")]
#[serde(rename_all = "PascalCase")]
pub enum TaskTriggerInfoType {
    DailyTrigger,
    WeeklyTrigger,
    IntervalTrigger,
    StartupTrigger,
}

impl TryFrom<String> for TaskTriggerInfoType {
    type Error = strum::ParseError;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        s.as_str().try_into()
    }
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskTriggerInfo {
    pub r#type: Option<String>,
    pub time_of_day_ticks: Option<i64>,
    pub interval_ticks: Option<i64>,
    pub day_of_week: Option<String>,
    pub max_runtime_ticks: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct TaskQueryResult {
    pub items: Vec<TaskInfo>,
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
    #[serde(alias = "playSessionId")]
    pub play_session_id: Option<String>,
    #[serde(alias = "mediaSourceId")]
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
    pub subtitle_method: Option<SubtitleDeliveryMethod>,
    pub max_streaming_bitrate: Option<i64>,
    pub transcode_reasons: Option<String>,
    /// Cumulative runtime ticks up to the start of this segment.
    #[serde(alias = "runtimeTicks")]
    pub runtime_ticks: Option<i64>,
    /// Length of this segment in ticks.
    #[serde(alias = "actualSegmentLengthTicks")]
    pub actual_segment_length_ticks: Option<i64>,
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

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHint {
    pub item_id: Uuid,
    pub name: Option<String>,
    pub matched_term: Option<String>,
    pub index_number: Option<i64>,
    pub production_year: Option<i64>,
    pub parent_index_number: Option<i64>,
    pub primary_image_tag: Option<String>,
    pub thumb_image_item_id: Option<Uuid>,
    pub thumb_image_tag: Option<String>,
    pub backdrop_image_item_id: Option<Uuid>,
    pub backdrop_image_tag: Option<String>,
    #[serde(rename = "Type")]
    pub type_: MediaType,
    pub is_folder: Option<bool>,
    pub run_time_ticks: Option<i64>,
    pub media_type: Option<String>,
    pub series_id: Option<Uuid>,
    pub series_name: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHintResult {
    pub search_hints: Vec<SearchHint>,
    pub total_record_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UtcTimeResponse {
    pub request_reception_time: DateTime<Utc>,
    pub response_transmission_time: DateTime<Utc>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct QueryFilters {
    pub genres: Option<Vec<NameIdPair>>,
    pub tags: Option<Vec<String>>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalIdInfo {
    pub name: String,
    pub key: String,
    #[serde(rename = "Type")]
    pub type_: Option<String>,
    pub url_format_string: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteImageInfo {
    pub provider_name: Option<String>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteImageResult {
    pub images: Option<Vec<RemoteImageInfo>>,
    pub total_record_count: i64,
    pub providers: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct SearchHintsQuery {
    pub search_term: Option<String>,
    pub start_index: Option<u32>,
    pub limit: Option<u32>,
    pub user_id: Option<Uuid>,
    #[serde(deserialize_with = "deserialize_media_types", default)]
    pub include_item_types: Option<Vec<MediaType>>,
}


#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct TunerHostInfo {
    pub id: Option<String>,
    pub url: Option<String>,
    #[serde(rename = "FriendlyName")]
    pub friendly_name: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub status: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EpgSourceInfo {
    pub id: Option<String>,
    pub name: String,
    pub url: SourceUrl,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ChannelEditorItem {
    pub id: String,
    pub name: String,
    pub custom_name: Option<String>,
    pub channel_number: Option<i64>,
    pub sort_order: Option<i64>,
    pub enabled: bool,
    pub logo: Option<String>,
    pub group: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PatchChannelRequest {
    pub enabled: Option<bool>,
    pub sort_order: Option<i64>,
    pub custom_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BulkChannelRequest {
    pub enabled: bool,
    pub search: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct IptvChannelsResult {
    pub items: Vec<ChannelEditorItem>,
    pub total_record_count: usize,
}

#[skip_serializing_none]
#[derive(Debug, Default, Deserialize, Serialize, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct ExternalUrl {
    pub name: Option<String>,
    pub url: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LyricLine {
    pub text: String,
    /// Start time in ticks (100-nanosecond units). None for unsynced lyrics.
    pub start: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LyricMetadata {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    /// Song length in ticks.
    pub length: Option<i64>,
    pub is_synced: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct LyricDto {
    pub metadata: LyricMetadata,
    pub lyrics: Vec<LyricLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemoteLyricInfoDto {
    pub id: String,
    pub provider_name: String,
    pub lyrics: LyricDto,
}
