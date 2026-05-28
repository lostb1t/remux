use chrono::{DateTime, NaiveDateTime, Utc};
use http::{HeaderValue, Method};
use nutype::nutype;
use serde::{Deserialize, Deserializer, Serialize};
use serde_alias::serde_alias;
use serde_aux::prelude::*;
use serde_with::{serde_as, skip_serializing_none};
use std::collections::HashMap;
use std::str::FromStr;
use uuid::Uuid;

use crate::stremio;
pub use crate::stremio::ResourceType;
use crate::{Auth, Body, Endpoint, RestClient};

#[derive(Clone, Debug)]
pub struct JellyfinAuth {
    pub client: String,
    pub device: String,
    pub device_id: String,
    pub version: String,
    pub token: Option<String>,
}

impl JellyfinAuth {
    pub fn new(device_id: impl Into<String>) -> Self {
        Self {
            client: "Remux Dashboard".to_string(),
            device: "Browser".to_string(),
            device_id: device_id.into(),
            version: "1.0.0".to_string(),
            token: None,
        }
    }

    pub fn with_token(mut self, token: impl Into<String>) -> Self {
        self.token = Some(token.into());
        self
    }
}

impl Auth for JellyfinAuth {
    fn apply(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let mut val = format!(
            r#"MediaBrowser Client="{}", Device="{}", DeviceId="{}", Version="{}""#,
            self.client, self.device, self.device_id, self.version
        );
        if let Some(token) = &self.token {
            val.push_str(&format!(r#", Token="{}""#, token));
        }
        match HeaderValue::from_str(&val) {
            Ok(v) => req.header(http::header::AUTHORIZATION, v),
            Err(_) => req,
        }
    }
}

pub fn client(base: &str) -> Result<RestClient, url::ParseError> {
    Ok(RestClient::new(base)?)
}

pub fn authed_client(
    base: &str,
    device_id: impl Into<String>,
    token: impl Into<String>,
) -> Result<RestClient<JellyfinAuth>, url::ParseError> {
    Ok(
        RestClient::new(base)?
            .with_auth(JellyfinAuth::new(device_id).with_token(token)),
    )
}

/// Form schema for one configurable option on an addon kind. The dashboard
/// renders the create/edit form generically by reading `Vec<AddonOption>`.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonOption {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    #[serde(default)]
    pub required: bool,
    pub default: Option<serde_json::Value>,
    #[serde(rename = "type")]
    pub kind: AddonOptionType,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum AddonOptionType {
    String,
    Url,
    Number {
        min: Option<i64>,
        max: Option<i64>,
    },
    Boolean,
    Password,
    Textarea,
    Select {
        options: Vec<AddonSelectOption>,
    },
    MultiSelect {
        options: Vec<AddonSelectOption>,
    },
    /// Repeatable input — e.g. multiple Deezer playlist IDs on one addon.
    StringList,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonSelectOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct AddonPresetRef {
    pub kind: String,
    #[serde(default)]
    pub config: serde_json::Value,
}

/// Static metadata describing one kind of addon. Returned by `GET /addon-kinds`
/// so the dashboard can populate the kind picker and config form.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonMetadata {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub supported_resources: Vec<ResourceType>,
    pub supported_types: Vec<MediaKind>,
    pub options: Vec<AddonOption>,
}

/// API representation of a stored addon instance.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonDto {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    /// User-enabled resources (subset of `supported_resources`).
    pub resources: Vec<ResourceType>,
    /// User-enabled content types (subset of `supported_types`). Empty = all types enabled.
    #[serde(default)]
    pub types: Vec<MediaKind>,
    pub enabled: bool,
    /// All resources the addon actually provides. For Stremio addons this is
    /// populated from the manifest; for other kinds it mirrors the static kind
    /// metadata. Used by the dashboard as the checkbox option list.
    #[serde(default)]
    pub supported_resources: Vec<ResourceType>,
    /// Content types the addon supports. For Stremio addons this comes from
    /// the manifest; for other kinds from the static preset metadata.
    #[serde(default)]
    pub supported_types: Vec<MediaKind>,
    pub priority: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

/// Create payload — `POST /addons`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAddonRequest {
    pub preset: AddonPresetRef,
    pub name: String,
    #[serde(default)]
    pub resources: Vec<ResourceType>,
    #[serde(default)]
    pub types: Vec<MediaKind>,
    #[serde(default)]
    pub priority: i64,
}

/// Update payload — `POST /addons/{id}`.
#[skip_serializing_none]
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAddonRequest {
    pub name: Option<String>,
    pub config: Option<serde_json::Value>,
    pub resources: Option<Vec<ResourceType>>,
    pub types: Option<Vec<MediaKind>>,
    pub enabled: Option<bool>,
    pub priority: Option<i64>,
}

/// One catalog exposed by an addon, merged with its current config state.
/// Returned by `GET /addons/{id}/catalogs`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonCatalogDto {
    /// The full catalog_id string: `addon:{addon_uuid}:{local_id}`.
    pub catalog_id: String,
    pub name: String,
    /// Whether this catalog is enabled for import.
    pub enabled: bool,
    /// Per-catalog item limit override.
    pub max_items: Option<i64>,
}

/// Per-catalog settings update — one entry in `POST /addons/{id}/catalogs`.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAddonCatalogRequest {
    /// The local catalog id (provider_catalog_id, e.g. `top/movie`).
    pub catalog_id: String,
    pub enabled: bool,
    pub max_items: Option<i64>,
}

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
}

impl DefaultWebClient {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Jellyfin => "jellyfin",
        }
    }

    pub fn from_str_lossy(_value: &str) -> Self {
        Self::Jellyfin
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
    pub date_added: DateTime<Utc>,
    pub authentication_token: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub app_name: Option<String>,
    pub app_version: Option<String>,
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
    pub catalog_max_items: Option<i64>,
    /// Number of items to process concurrently during metadata fetch (default: 5).
    #[default(Some(5_i64))]
    pub meta_concurrency: Option<i64>,
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
    pub jellyfin_url: Option<String>,
    pub jellyfin_api_key: Option<String>,
    /// Kinds that use remote (addon) search. None = all remote-capable kinds enabled.
    /// Values are snake_case MediaKind strings: "movie", "series", "track", "album", "artist", "person".
    pub search_remote_enabled: Option<Vec<String>>,
    /// Probe timeout in seconds for HTTP/local streams (default: 20).
    #[default(Some(20_i64))]
    pub probe_timeout_secs: Option<i64>,
    /// Probe timeout in seconds for P2P (torrent) streams (default: 60).
    #[default(Some(60_i64))]
    pub probe_timeout_p2p_secs: Option<i64>,
    /// When probe fails, automatically try the next stream with matching resolution and type.
    #[default(Some(true))]
    pub auto_next_stream_on_probe_fail: Option<bool>,
    /// Maximum number of alternative streams to try before giving up (default: 3).
    #[default(Some(3_i64))]
    pub max_probe_fallback_streams: Option<i64>,
    /// Show streams that don't match any group individually (default true).
    #[default(Some(true))]
    pub stream_groups_show_ungrouped: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum HardwareAccelerationType {
    /// Software encoding only.
    #[default]
    None,
    Vaapi,
    Nvenc,
    Qsv,
    Amf,
    VideoToolbox,
    V4l2m2m,
    Rkmpp,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct EncodingOptions {
    #[default(Some("fast".to_string()))]
    pub encoding_preset: Option<String>,
    #[default(Some(HardwareAccelerationType::None))]
    pub hardware_acceleration_type: Option<HardwareAccelerationType>,
    /// VAAPI render device path (used when hardware_acceleration_type is vaapi or qsv).
    #[default(Some("/dev/dri/renderD128".to_string()))]
    pub vaapi_device: Option<String>,
    /// VAAPI driver name to pass via `driver=` in `-init_hw_device vaapi=...`.
    /// Auto-detected at startup: "iHD" for Intel, empty for others.
    #[default(None)]
    pub vaapi_driver: Option<String>,
    /// When true, the server probes available hardware at startup and saves the
    /// detected type to hardware_acceleration_type automatically.
    #[default(Some(true))]
    pub auto_detect_hardware_acceleration: Option<bool>,
    /// Software HDR→SDR tone mapping via the tonemapx filter (CPU).
    #[default(Some(false))]
    pub enable_tonemapping: Option<bool>,
    /// Hardware HDR→SDR tone mapping via tonemap_vaapi (Intel VAAPI/QSV only).
    #[default(Some(false))]
    pub enable_vpp_tonemapping: Option<bool>,
    /// Algorithm used by tonemapx: hable, reinhard, mobius, bt2390, bt2446a, none.
    #[default(Some("hable".to_string()))]
    pub tonemapping_algorithm: Option<String>,
    /// Desaturation coefficient for tonemapx (0.0 = disabled).
    #[default(Some(0.0_f32))]
    pub tonemapping_desat: Option<f32>,
    /// Peak luminance for tonemapx (nits). 0 = auto.
    #[default(Some(0.0_f32))]
    pub tonemapping_peak: Option<f32>,
    /// Allow HEVC (H.265) hardware/software encoding.
    #[default(Some(false))]
    pub allow_hevc_encoding: Option<bool>,
    /// Allow AV1 hardware/software encoding.
    #[default(Some(false))]
    pub allow_av1_encoding: Option<bool>,
    /// CRF quality level for software H.264 (libx264). Lower = better quality.
    #[default(Some(23u32))]
    pub h264_crf: Option<u32>,
    /// CRF quality level for software H.265 (libx265). Lower = better quality.
    #[default(Some(28u32))]
    pub h265_crf: Option<u32>,
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

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ParentalRatingScore {
    #[serde(alias = "score")]
    pub score: i32,
    #[serde(alias = "subScore")]
    pub sub_score: Option<i32>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ParentalRating {
    pub name: String,
    pub value: Option<i32>,
    pub rating_score: Option<ParentalRatingScore>,
}

impl ParentalRating {
    pub fn scored(name: &str, score: i32, sub_score: Option<i32>) -> Self {
        let rating_score = ParentalRatingScore { score, sub_score };
        Self {
            name: name.to_string(),
            value: Some(score),
            rating_score: Some(rating_score),
        }
    }

    pub fn unrated(name: &str) -> Self {
        Self {
            name: name.to_string(),
            value: None,
            rating_score: None,
        }
    }
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
    pub remux_version: String,
    pub operating_system: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CastReceiverApplication {
    pub id: String,
    pub name: String,
}

#[skip_serializing_none]
#[derive(Debug, Serialize, Deserialize, default2::Default)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub operating_system_display_name: Option<String>,
    pub product_name: String,
    pub version: String,
    pub remux_version: String,
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
    pub operating_system: Option<String>,
    pub id: Option<String>,
    #[default(vec![
        CastReceiverApplication { id: "F007D354".to_string(), name: "Stable".to_string() },
        CastReceiverApplication { id: "6F511C87".to_string(), name: "Unstable".to_string() },
    ])]
    pub cast_receiver_applications: Vec<CastReceiverApplication>,
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
    pub sort_order: Option<i64>,
}

/// A single catalog discovered from a user-configured Stremio manifest.
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct StremioManifestCatalogInfo {
    /// `"{manifest_url}||{kind}:{catalog_id}"` — the provider_catalog_id understood by StremioProvider.
    pub stremio_id: String,
    pub manifest_url: String,
    pub manifest_name: String,
    pub name: String,
    pub enabled: Option<bool>,
    pub max_items: Option<i64>,
    /// UUID of the DB media row if the catalog has been persisted.
    pub media_id: Option<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateStremioManifestUrlsPayload {
    pub urls: Vec<String>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateStremioCatalogSettingsPayload {
    pub stremio_id: String,
    pub enabled: bool,
    pub max_items: Option<i64>,
    pub name: Option<String>,
}

/// A user-created playlist catalog (e.g. a Deezer playlist).
#[skip_serializing_none]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistInfo {
    pub id: String,
    pub title: String,
    pub provider: String,
    pub provider_id: String,
    pub enabled: Option<bool>,
    pub max_items: Option<i64>,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreatePlaylistPayload {
    pub title: String,
    pub provider: String,
    /// Playlist URL (e.g. `https://www.deezer.com/playlist/12345`) or bare ID.
    pub url: String,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePlaylistSettingsPayload {
    pub enabled: bool,
    pub max_items: Option<i64>,
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
    pub sort_order: Option<i64>,
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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
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
#[serde(rename_all = "PascalCase")]
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
    /// Internal server-side constraint. This is not a Jellyfin query parameter.
    #[serde(skip)]
    pub promoted: Option<bool>,
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
    pub start_item_id: Option<Uuid>,
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

#[derive(Default, Debug, Deserialize, Clone)]
#[serde(rename_all = "PascalCase", default)]
pub struct DirectPlayProfile {
    pub container: Option<String>,
    pub audio_codec: Option<String>,
    pub video_codec: Option<String>,
    #[serde(rename = "Type")]
    pub type_: Option<String>, // "Video", "Audio", etc.
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
    /// Scale down to fit within box width, no upscale, maintain AR.
    pub fill_width: Option<u32>,
    /// Scale down to fit within box height, no upscale, maintain AR.
    pub fill_height: Option<u32>,
    /// Resize to exact width (maintains AR when height is omitted).
    pub width: Option<u32>,
    /// Resize to exact height (maintains AR when width is omitted).
    pub height: Option<u32>,
    /// Cap width; scale down if needed, maintain AR.
    pub max_width: Option<u32>,
    /// Cap height; scale down if needed, maintain AR.
    pub max_height: Option<u32>,
    /// JPEG quality 0–100 (server default 90).
    pub quality: Option<u8>,
    /// Gaussian blur sigma in pixels.
    pub blur: Option<u32>,
    /// Hex background color to composite behind transparent images.
    pub background_color: Option<String>,
    /// Output format: "jpg", "jpeg", "png". Defaults to jpeg.
    pub format: Option<String>,
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

#[skip_serializing_none]
#[derive(Default, Deserialize, Serialize, Clone, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct ThemeMediaResult {
    pub owner_id: String,
    #[serde(default)]
    pub items: Vec<BaseItemDto>,
    pub start_index: u32,
    pub total_record_count: i64,
}

#[derive(Clone, PartialEq, Eq)]
pub enum TranscodeReason {
    ContainerNotSupported(String),
    VideoCodecNotSupported(String),
    AudioCodecNotSupported(String),
    SubtitleCodecNotSupported(String),
    VideoRangeTypeNotSupported(String),
    VideoCodecTagNotSupported(String),
    ContainerBitrateExceedsLimit,
}

impl TranscodeReason {
    pub fn name(&self) -> &'static str {
        match self {
            Self::ContainerNotSupported(_) => "ContainerNotSupported",
            Self::VideoCodecNotSupported(_) => "VideoCodecNotSupported",
            Self::AudioCodecNotSupported(_) => "AudioCodecNotSupported",
            Self::SubtitleCodecNotSupported(_) => "SubtitleCodecNotSupported",
            Self::VideoRangeTypeNotSupported(_) => "VideoRangeTypeNotSupported",
            Self::VideoCodecTagNotSupported(_) => "VideoCodecTagNotSupported",
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
            Self::SubtitleCodecNotSupported(d) => {
                write!(f, "SubtitleCodecNotSupported({d})")
            }
            Self::VideoRangeTypeNotSupported(d) => {
                write!(f, "VideoRangeTypeNotSupported({d})")
            }
            Self::VideoCodecTagNotSupported(d) => {
                write!(f, "VideoCodecTagNotSupported({d})")
            }
            Self::ContainerBitrateExceedsLimit => {
                write!(f, "ContainerBitrateExceedsLimit")
            }
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
        Some(
            self.0
                .iter()
                .map(|r| r.name())
                .collect::<Vec<_>>()
                .join(","),
        )
    }

    pub fn from_query_value(s: &str) -> Self {
        let mut out = Self::default();
        for part in s.split(',') {
            let reason = match part.trim() {
                "ContainerNotSupported" => {
                    Some(TranscodeReason::ContainerNotSupported(String::new()))
                }
                "VideoCodecNotSupported" => {
                    Some(TranscodeReason::VideoCodecNotSupported(String::new()))
                }
                "AudioCodecNotSupported" => {
                    Some(TranscodeReason::AudioCodecNotSupported(String::new()))
                }
                "SubtitleCodecNotSupported" => {
                    Some(TranscodeReason::SubtitleCodecNotSupported(String::new()))
                }
                "VideoRangeTypeNotSupported" => {
                    Some(TranscodeReason::VideoRangeTypeNotSupported(String::new()))
                }
                "VideoCodecTagNotSupported" => {
                    Some(TranscodeReason::VideoCodecTagNotSupported(String::new()))
                }
                "ContainerBitrateExceedsLimit" => {
                    Some(TranscodeReason::ContainerBitrateExceedsLimit)
                }
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
    pub protocol: MediaProtocol,
    #[default(false)]
    pub read_at_native_framerate: bool,
    pub required_http_headers: Option<HashMap<String, String>>,
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
    pub type_: MediaSourceType,
    #[default(false)]
    pub use_most_compatible_transcoding_profile: bool,
    //  pub video3_d_format: Option<Video3DFormat>,
    pub video_type: VideoType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub segments: Option<MediaSegments>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remux: Option<MediaSourceRemuxInfo>,
    #[serde(default, skip_serializing_if = "TranscodeReasons::is_empty")]
    pub transcoding_reasons: TranscodeReasons,
}

#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSourceRemuxInfo {
    pub provider_info: Option<serde_json::Value>,
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

#[derive(
    Debug,
    Clone,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase", ascii_case_insensitive)]
pub enum ImageType {
    Primary,
    Backdrop,
    Logo,
    #[strum(serialize = "logo")]
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
    pub max_parental_rating: Option<i32>,
    pub max_parental_sub_rating: Option<i32>,
    #[serde(default)]
    pub blocked_tags: Vec<String>,
    #[serde(default)]
    pub allowed_tags: Vec<String>,
    /// Per-user filter rules applied on every item query (same engine as smart collections).
    pub filter_rules: Option<CollectionFilter>,
    /// Per-user stream filter applied during playback. Restricts which streams are offered.
    pub stream_filter: Option<StreamFilter>,
    /// When false, search falls back to local DB only (no addon/remote sources).
    #[serde(default = "default_true")]
    #[default(true)]
    pub enable_remote_search: bool,
    #[default(true)]
    pub enable_user_preference_access: bool,
    #[serde(default)]
    pub access_schedules: Vec<String>,
    #[serde(default)]
    pub block_unrated_items: Vec<String>,
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
    #[serde(default)]
    pub enable_content_deletion_from_folders: Vec<String>,
    #[default(true)]
    pub enable_content_downloading: bool,
    #[default(true)]
    pub enable_sync_transcoding: bool,
    #[default(true)]
    pub enable_media_conversion: bool,
    #[serde(default)]
    pub enabled_devices: Vec<String>,
    #[default(true)]
    pub enable_all_devices: bool,
    #[serde(default)]
    pub enabled_channels: Vec<String>,
    #[default(true)]
    pub enable_all_channels: bool,
    #[serde(default)]
    pub enabled_folders: Vec<String>,
    #[default(true)]
    pub enable_all_folders: bool,
    pub invalid_login_attempt_count: i64,
    #[default(-1)]
    pub login_attempts_before_lockout: i64,
    #[serde(default, deserialize_with = "deserialize_max_sessions")]
    pub max_active_sessions: i64,
    #[default(true)]
    pub enable_public_sharing: bool,
    #[serde(default)]
    pub blocked_media_folders: Vec<String>,
    #[serde(default)]
    pub blocked_channels: Vec<String>,
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

fn default_true() -> bool {
    true
}

fn deserialize_max_sessions<'de, D>(d: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let n: Option<i64> = Option::deserialize(d)?;
    Ok(n.unwrap_or(0).max(0))
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

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MediaProtocol {
    #[default]
    File,
    Http,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MediaSourceType {
    #[default]
    Default,
    Placeholder,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum VideoType {
    #[default]
    VideoFile,
    BluRay,
    Dvd,
    Iso,
    HdDvd,
}

#[derive(Default, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum LocationType {
    #[default]
    FileSystem,
    Remote,
    Virtual,
    Offline,
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
pub enum MediaKind {
    #[default]
    Movie,
    Series,
    Season,
    Episode,
    Collection,
    Folder,
    Genre,
    Person,
    Studio,
    Stream,
    TvChannel,
    TvProgram,
    Track,
    Album,
    Artist,
    Playlist,
}

impl From<stremio::MediaType> for MediaKind {
    fn from(t: stremio::MediaType) -> Self {
        match t {
            stremio::MediaType::Movie => Self::Movie,
            stremio::MediaType::Series | stremio::MediaType::Tv => Self::Series,
            stremio::MediaType::Album => Self::Album,
            stremio::MediaType::Artist => Self::Artist,
            stremio::MediaType::Track => Self::Track,
            stremio::MediaType::Events => Self::TvProgram,
            stremio::MediaType::Unknown(s) => match s.as_str() {
                "episode" => Self::Episode,
                "season" => Self::Season,
                "person" => Self::Person,
                "genre" => Self::Genre,
                "studio" => Self::Studio,
                "collection" => Self::Collection,
                "folder" => Self::Folder,
                "stream" => Self::Stream,
                "playlist" => Self::Playlist,
                _ => Self::Movie,
            },
        }
    }
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
    Genre {
        op: SetOp,
        values: Vec<String>,
    },
    Year {
        op: NumericOp,
        value: i64,
    },
    RatingAudience {
        op: NumericOp,
        value: f64,
    },
    RatingCritic {
        op: NumericOp,
        value: f64,
    },
    ParentalRating {
        op: NumericOp,
        value: i64,
    },
    Certification {
        op: SetOp,
        values: Vec<String>,
    },
    Tag {
        op: SetOp,
        values: Vec<String>,
    },
    Studio {
        op: SetOp,
        values: Vec<String>,
    },
    HasTrailer {
        value: bool,
    },
    Country {
        op: SetOp,
        values: Vec<String>,
    },
    Person {
        op: SetOp,
        values: Vec<String>,
    },
    /// Kept for backward compatibility with rows stored before the catalog→collection rename.
    #[serde(alias = "catalog")]
    Collection {
        op: SetOp,
        values: Vec<String>,
    },
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
    #[serde(default, deserialize_with = "deserialize_filter_rules")]
    pub rules: Vec<FilterRule>,
}

fn deserialize_filter_rules<'de, D>(
    deserializer: D,
) -> Result<Vec<FilterRule>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Vec<serde_json::Value> = Vec::deserialize(deserializer)?;
    Ok(raw
        .into_iter()
        .filter_map(|v| serde_json::from_value::<FilterRule>(v).ok())
        .collect())
}

#[skip_serializing_none]
#[derive(Default, Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct RemuxInfo {
    pub collection_kind: Option<RemuxCollectionKind>,
    pub collection_media_kind: Option<MediaKind>,
    pub collection_max_items: Option<i64>,
    pub smart_filter: Option<CollectionFilter>,
    pub promoted: Option<bool>,
    pub digital_release_date: Option<DateTime<Utc>>,
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
    #[serde(default)]
    pub external_urls: Vec<ExternalUrl>,
    pub media_sources: Option<Vec<MediaSourceInfo>>,
    pub critic_rating: Option<f64>,
    pub production_locations: Option<Vec<String>>,
    pub path: Option<String>,
    pub official_rating: Option<String>,
    pub custom_rating: Option<String>,
    pub channel_id: Option<String>,
    pub channel_name: Option<String>,
    pub overview: Option<String>,
    #[serde(default)]
    pub taglines: Vec<String>,
    #[serde(default)]
    pub genres: Vec<String>,
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
    #[serde(default)]
    pub people: Vec<BaseItemPerson>,
    #[serde(default)]
    pub studios: Vec<NameIdPair>,
    #[serde(default)]
    pub genre_items: Vec<NameIdPair>,
    pub parent_logo_item_id: Option<String>,
    pub parent_backdrop_item_id: Option<String>,
    pub parent_backdrop_image_tags: Option<Vec<String>>,
    #[serde(default)]
    pub local_trailer_count: i64,
    #[serde(default)]
    pub remote_trailers: Vec<ExternalUrl>,
    pub user_data: Option<UserItemDataDto>,
    pub recursive_item_count: Option<i64>,
    pub child_count: Option<i64>,
    pub series_name: Option<String>,
    pub series_id: Option<Uuid>,
    pub season_id: Option<Uuid>,
    #[serde(default)]
    pub special_feature_count: i64,
    pub display_preferences_id: Option<String>,
    pub status: Option<Status>,
    pub air_time: Option<String>,
    pub air_days: Option<Vec<String>>,
    #[serde(default)]
    pub tags: Vec<String>,

    // this is fucking weird. And its used.
    // anyway we set it to poster format by default
    #[default(Some(0.6))]
    pub primary_image_aspect_ratio: Option<f32>,
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
    pub video_type: Option<VideoType>,
    pub part_count: Option<i64>,
    pub media_source_count: Option<i64>,
    pub image_tags: Option<ImageTags>,
    #[serde(default)]
    pub backdrop_image_tags: Vec<String>,
    pub image_blur_hashes: Option<ImageBlurHashes>,
    pub screenshot_image_tags: Option<Vec<String>>,
    pub parent_thumb_item_id: Option<String>,
    pub parent_thumb_image_tag: Option<String>,
    pub parent_primary_image_item_id: Option<String>,
    pub parent_primary_image_tag: Option<String>,
    //pub chapters: Option<Vec<ChapterInfo>>,
    #[default(LocationType::FileSystem)]
    pub location_type: LocationType,
    pub iso_type: Option<String>,
    #[default(MediaType::Unknown)]
    pub media_type: MediaType,
    pub end_date: Option<String>,
    #[serde(default)]
    pub locked_fields: Vec<String>,
    pub trailer_count: Option<i64>,
    pub movie_count: Option<i64>,
    pub series_count: Option<i64>,
    pub program_count: Option<i64>,
    pub episode_count: Option<i64>,
    pub song_count: Option<i64>,
    pub album_count: Option<i64>,
    pub artist_count: Option<i64>,
    pub music_video_count: Option<i64>,
    pub lock_data: bool,
    #[default(true)]
    pub enable_media_source_display: bool,
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
#[derive(Debug, Clone, default2::Default, Serialize, Deserialize)]
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
    #[default("RepeatNone".to_string())]
    pub repeat_mode: String,
    #[default("Default".to_string())]
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
    #[serde(default)]
    pub now_playing_queue: Vec<QueueItem>,
    #[serde(default)]
    pub now_playing_queue_full_items: Vec<BaseItemDto>,
    pub has_custom_device_name: bool,
    pub playlist_item_id: Option<String>,
    pub server_id: String,
    pub user_primary_image_tag: Option<String>,
    pub supported_commands: Vec<String>,
}

#[skip_serializing_none]
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
#[serde(default, rename_all = "PascalCase")]
pub struct PlaybackInfo {
    /// Optional event kind; when absent (legacy payloads) this will be None.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "EventType", default)]
    pub event_type: Option<PlaybackEventKind>,

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
    pub next_media_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlaybackEventKind {
    Start,
    Progress,
    Stop,
}

// Backwards-compatibility type aliases so other crates referring to the old
// names continue to compile until callers are updated. These are deprecated
// but harmless and allow a single type to replace the three legacy structs.
#[deprecated(note = "Use PlaybackInfo instead")]
pub type PlaybackStartInfo = PlaybackInfo;
#[deprecated(note = "Use PlaybackInfo instead")]
pub type PlaybackProgressInfo = PlaybackInfo;
#[deprecated(note = "Use PlaybackInfo instead")]
pub type PlaybackStopInfo = PlaybackInfo;

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
    ChannelOrder,
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
)]
#[strum(serialize_all = "PascalCase")]
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
    AlternateMediaSources,
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
    AlbumArtist,
    AlbumArtists,
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

impl std::str::FromStr for ItemFields {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        use serde::de::IntoDeserializer;
        let d: serde::de::value::StrDeserializer<serde::de::value::Error> =
            s.into_deserializer();
        Ok(ItemFields::deserialize(d).unwrap_or(ItemFields::Unknown))
    }
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

impl VideoRangeType {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "Unknown",
            Self::Sdr => "SDR",
            Self::Hdr10 => "HDR10",
            Self::Hlg => "HLG",
            Self::Dovi => "DOVI",
            Self::DoviWithHdr10 => "DOVIWithHDR10",
            Self::DoviWithHlg => "DOVIWithHLG",
            Self::DoviWithSdr => "DOVIWithSDR",
            Self::Hdr10Plus => "HDR10Plus",
        }
    }
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
    pub short_description: Option<String>,
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
    pub country: Option<String>,
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

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Serialize,
    Deserialize,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "PascalCase")]
pub enum MediaSegmentType {
    Unknown = 0,
    Commercial = 1,
    Preview = 2,
    Recap = 3,
    Outro = 4,
    Intro = 5,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Segment {
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaSegments {
    pub intro: Option<Segment>,
    pub outro: Option<Segment>,
    pub recap: Option<Segment>,
    pub preview: Option<Segment>,
    pub commercial: Option<Segment>,
}

impl MediaSegments {
    pub fn is_empty(&self) -> bool {
        self.intro.is_none()
            && self.outro.is_none()
            && self.recap.is_none()
            && self.preview.is_none()
            && self.commercial.is_none()
    }

    /// Fill types that are `None` in `self` from `other`. Existing values are kept.
    pub fn merge_from(&mut self, other: MediaSegments) {
        if self.intro.is_none() {
            self.intro = other.intro;
        }
        if self.outro.is_none() {
            self.outro = other.outro;
        }
        if self.recap.is_none() {
            self.recap = other.recap;
        }
        if self.preview.is_none() {
            self.preview = other.preview;
        }
        if self.commercial.is_none() {
            self.commercial = other.commercial;
        }
    }

    /// Expand into a flat list of `(type, segment)` pairs, ordered by start tick.
    pub fn to_pairs(&self) -> Vec<(MediaSegmentType, &Segment)> {
        let mut pairs: Vec<(MediaSegmentType, &Segment)> = [
            self.intro.as_ref().map(|s| (MediaSegmentType::Intro, s)),
            self.outro.as_ref().map(|s| (MediaSegmentType::Outro, s)),
            self.recap.as_ref().map(|s| (MediaSegmentType::Recap, s)),
            self.preview
                .as_ref()
                .map(|s| (MediaSegmentType::Preview, s)),
            self.commercial
                .as_ref()
                .map(|s| (MediaSegmentType::Commercial, s)),
        ]
        .into_iter()
        .flatten()
        .collect();
        pairs.sort_by_key(|(_, s)| s.start_ticks);
        pairs
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MediaSegmentDto {
    pub id: Uuid,
    pub item_id: Uuid,
    pub r#type: MediaSegmentType,
    pub start_ticks: i64,
    pub end_ticks: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PlaylistCreationResult {
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CreatePlaylistDto {
    pub name: Option<String>,
    #[serde(default)]
    pub ids: Vec<Uuid>,
    pub user_id: Option<Uuid>,
    pub media_type: Option<MediaType>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct UpdatePlaylistDto {
    pub name: Option<String>,
    pub ids: Option<Vec<Uuid>>,
}

impl Endpoint for PublicSystemInfo {
    type Output = PublicSystemInfo;

    fn path(&self) -> String {
        "/system/info/public".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetSessions {
    pub active_within_seconds: Option<i64>,
}

impl Endpoint for GetSessions {
    type Output = Vec<SessionInfoDto>;

    fn path(&self) -> String {
        "/sessions".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        match self.active_within_seconds {
            Some(s) => vec![("activeWithinSeconds".into(), s.to_string())],
            None => vec![],
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetScheduledTasks {
    pub is_hidden: Option<bool>,
}

impl Endpoint for GetScheduledTasks {
    type Output = Vec<TaskInfo>;

    fn path(&self) -> String {
        "/scheduledtasks".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        match self.is_hidden {
            Some(v) => vec![("isHidden".into(), v.to_string())],
            None => vec![],
        }
    }
}

#[derive(Debug, Clone)]
pub struct GetTask {
    pub task_id: String,
}

impl Endpoint for GetTask {
    type Output = TaskInfo;

    fn path(&self) -> String {
        format!("/scheduledtasks/{}", self.task_id)
    }
}

#[derive(Debug, Clone)]
pub struct GetJellyfinItemsByIds {
    pub ids: Vec<String>,
}

impl Endpoint for GetJellyfinItemsByIds {
    type Output = QueryResult<JellyfinItem>;

    fn path(&self) -> String {
        "/Items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("Ids".into(), self.ids.join(",")),
            ("Fields".into(), "ProviderIds".into()),
        ]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetJellyfinUsers;

impl Endpoint for GetJellyfinUsers {
    type Output = Vec<JellyfinUserDto>;

    fn path(&self) -> String {
        "/Users".into()
    }
}

#[derive(Debug, Clone)]
pub struct GetJellyfinUserItems {
    pub user_id: String,
    pub filter: &'static str,
}

impl Endpoint for GetJellyfinUserItems {
    type Output = QueryResult<JellyfinItem>;

    fn path(&self) -> String {
        format!("/Users/{}/Items", self.user_id)
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("Recursive".into(), "true".into()),
            (
                "Fields".into(),
                "ProviderIds,SeriesProviderIds,UserData,SeriesId".into(),
            ),
            ("IncludeItemTypes".into(), "Movie,Episode".into()),
            ("Filters".into(), self.filter.into()),
        ]
    }
}

#[derive(Debug, Clone)]
pub struct StartTask {
    pub task_id: String,
}

impl Endpoint for StartTask {
    type Output = ();

    fn path(&self) -> String {
        format!("/scheduledtasks/running/{}", self.task_id)
    }

    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone)]
pub struct StopTask {
    pub task_id: String,
}

impl Endpoint for StopTask {
    type Output = ();

    fn path(&self) -> String {
        format!("/scheduledtasks/running/{}", self.task_id)
    }

    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone)]
pub struct UpdateTaskTriggers {
    pub task_id: String,
    pub triggers: Vec<TaskTriggerInfo>,
}

impl Endpoint for UpdateTaskTriggers {
    type Output = ();
    fn path(&self) -> String {
        format!("/scheduledtasks/{}/triggers", self.task_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.triggers).unwrap_or_default())
    }
}

// --- Stremio endpoints ---

#[derive(Debug, Clone, Default)]
pub struct GetStremioCatalogs;

impl Endpoint for GetStremioCatalogs {
    type Output = Vec<StremioManifestCatalogInfo>;
    fn path(&self) -> String {
        "/stremio/catalogs".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateStremioCatalogSettings {
    pub payload: UpdateStremioCatalogSettingsPayload,
}

impl Endpoint for UpdateStremioCatalogSettings {
    type Output = ();
    fn path(&self) -> String {
        "/stremio/catalogs".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

// --- Catalog playlist (remote import source) endpoints ---

#[derive(Debug, Clone, Default)]
pub struct GetCatalogPlaylists;

impl Endpoint for GetCatalogPlaylists {
    type Output = Vec<PlaylistInfo>;
    fn path(&self) -> String {
        "/catalog-playlists".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateCatalogPlaylist {
    pub payload: CreatePlaylistPayload,
}

impl Endpoint for CreateCatalogPlaylist {
    type Output = PlaylistInfo;
    fn path(&self) -> String {
        "/catalog-playlists".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteCatalogPlaylist {
    pub id: String,
}

impl Endpoint for DeleteCatalogPlaylist {
    type Output = ();
    fn path(&self) -> String {
        format!("/catalog-playlists/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone)]
pub struct UpdateCatalogPlaylistSettings {
    pub id: String,
    pub payload: UpdatePlaylistSettingsPayload,
}

impl Endpoint for UpdateCatalogPlaylistSettings {
    type Output = ();
    fn path(&self) -> String {
        format!("/catalog-playlists/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetItems {
    pub include_item_types: Vec<String>,
    pub recursive: bool,
    pub sort_by: Option<Vec<ItemSortBy>>,
    pub sort_order: Option<Vec<SortOrder>>,
}

impl Endpoint for GetItems {
    type Output = QueryResult<BaseItemDto>;

    fn path(&self) -> String {
        "/items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        let mut q = vec![];
        if !self.include_item_types.is_empty() {
            q.push(("IncludeItemTypes".into(), self.include_item_types.join(",")));
        }
        if self.recursive {
            q.push(("Recursive".into(), "true".into()));
        }
        if let Some(sb) = &self.sort_by {
            q.push((
                "SortBy".into(),
                sb.iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        if let Some(so) = &self.sort_order {
            q.push((
                "SortOrder".into(),
                so.iter()
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            ));
        }
        q
    }
}

/// Local DB title-contains search for a specific media kind (Genre, Studio, Person, …).
/// Sends `SearchTerm=local:{query}` so the server skips AIO and queries the DB directly.
#[derive(Debug, Clone)]
pub struct GetLocalSuggestions {
    pub kind: String,
    pub search_term: String,
}

impl Endpoint for GetLocalSuggestions {
    type Output = QueryResult<BaseItemDto>;

    fn path(&self) -> String {
        "/items".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![
            ("IncludeItemTypes".into(), self.kind.clone()),
            ("SearchTerm".into(), format!("local:{}", self.search_term)),
            ("Limit".into(), "25".into()),
        ]
    }
}

/// Fetch distinct tag suggestions from the local DB, optionally filtered.
#[derive(Debug, Clone, Default)]
pub struct GetTagSuggestions {
    pub search_term: String,
}

impl Endpoint for GetTagSuggestions {
    type Output = Vec<String>;

    fn path(&self) -> String {
        "/items/tags".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        if self.search_term.is_empty() {
            vec![]
        } else {
            vec![("SearchTerm".into(), self.search_term.clone())]
        }
    }
}

/// Fetch distinct certification values, optionally filtered.
#[derive(Debug, Clone, Default)]
pub struct GetCertificationSuggestions {
    pub search_term: String,
}

impl Endpoint for GetCertificationSuggestions {
    type Output = Vec<String>;

    fn path(&self) -> String {
        "/items/certifications".into()
    }

    fn query(&self) -> Vec<(String, String)> {
        if self.search_term.is_empty() {
            vec![]
        } else {
            vec![("SearchTerm".into(), self.search_term.clone())]
        }
    }
}

/// Fetch the full ISO 3166-1 country list from the locale endpoint.
/// The client filters by name/alpha2 locally (only ~250 entries).
#[derive(Debug, Clone, Default)]
pub struct GetCountries;

impl Endpoint for GetCountries {
    type Output = Vec<CountryInfo>;

    fn path(&self) -> String {
        "/localization/countries".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetParentalRatings;

impl Endpoint for GetParentalRatings {
    type Output = Vec<ParentalRating>;

    fn path(&self) -> String {
        "/localization/parentalratings".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetVirtualFolders;

impl Endpoint for GetVirtualFolders {
    type Output = Vec<VirtualFolderInfo>;
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateVirtualFolder {
    pub payload: CreateVirtualFolderPayload,
}

impl Endpoint for CreateVirtualFolder {
    type Output = VirtualFolderInfo;
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateVirtualFolder {
    pub payload: UpdateVirtualFolderPayload,
}

impl Endpoint for UpdateVirtualFolder {
    type Output = ();
    fn path(&self) -> String {
        "/library/virtualfolders/LibraryOptions".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteVirtualFolder {
    pub name: String,
}

impl Endpoint for DeleteVirtualFolder {
    type Output = ();
    fn path(&self) -> String {
        "/library/virtualfolders".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("name".into(), self.name.clone())]
    }
}

#[derive(Debug, Clone)]
pub struct PatchItem {
    pub item_id: String,
    pub payload: PatchItemPayload,
}

impl Endpoint for PatchItem {
    type Output = ();
    fn path(&self) -> String {
        format!("/items/{}", self.item_id)
    }
    fn method(&self) -> Method {
        Method::PATCH
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

/// Upload an image for a library item (POST /Items/{id}/Images/{type}).
/// Bytes are sent as-is with the given content-type header.
#[derive(Debug, Clone)]
pub struct UploadItemImage {
    pub item_id: String,
    pub image_type: String,
    pub bytes: Vec<u8>,
    pub content_type: &'static str,
}

impl Endpoint for UploadItemImage {
    type Output = ();
    fn path(&self) -> String {
        format!("/Items/{}/Images/{}", self.item_id, self.image_type)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn headers(&self) -> http::HeaderMap {
        let mut map = http::HeaderMap::new();
        if let Ok(v) = http::HeaderValue::from_str(self.content_type) {
            map.insert(http::header::CONTENT_TYPE, v);
        }
        map
    }
    fn body(&self) -> Body {
        Body::Bytes(self.bytes.clone())
    }
}

/// Delete an image for a library item (DELETE /Items/{id}/Images/{type}).
#[derive(Debug, Clone)]
pub struct DeleteItemImage {
    pub item_id: String,
    pub image_type: String,
}

impl Endpoint for DeleteItemImage {
    type Output = ();
    fn path(&self) -> String {
        format!("/Items/{}/Images/{}", self.item_id, self.image_type)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetSystemConfiguration;

impl Endpoint for GetSystemConfiguration {
    type Output = ServerConfiguration;
    fn path(&self) -> String {
        "/system/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateSystemConfiguration {
    pub config: ServerConfiguration,
}

impl Endpoint for UpdateSystemConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/system/configuration".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetEncodingConfiguration;

impl Endpoint for GetEncodingConfiguration {
    type Output = EncodingOptions;
    fn path(&self) -> String {
        "/system/configuration/encoding".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateEncodingConfiguration {
    pub config: EncodingOptions,
}

impl Endpoint for UpdateEncodingConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/system/configuration/encoding".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetBrandingConfiguration;

impl Endpoint for GetBrandingConfiguration {
    type Output = BrandingOptions;
    fn path(&self) -> String {
        "/branding/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateBrandingConfiguration {
    pub config: BrandingOptions,
}

impl Endpoint for UpdateBrandingConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/system/configuration/branding".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetStartupConfiguration;

impl Endpoint for GetStartupConfiguration {
    type Output = StartupConfiguration;
    fn path(&self) -> String {
        "/startup/configuration".into()
    }
}

#[derive(Debug, Clone)]
pub struct PostStartupConfiguration {
    pub payload: StartupConfiguration,
}

impl Endpoint for PostStartupConfiguration {
    type Output = ();
    fn path(&self) -> String {
        "/startup/configuration".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct PostStartupUser {
    pub payload: StartupUser,
}

impl Endpoint for PostStartupUser {
    type Output = ();
    fn path(&self) -> String {
        "/startup/user".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupRemoteAccess;

impl Endpoint for PostStartupRemoteAccess {
    type Output = ();
    fn path(&self) -> String {
        "/startup/remoteaccess".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone, Default)]
pub struct PostStartupComplete;

impl Endpoint for PostStartupComplete {
    type Output = ();
    fn path(&self) -> String {
        "/startup/complete".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetItemCounts;

impl Endpoint for GetItemCounts {
    type Output = ItemCounts;
    fn path(&self) -> String {
        "/items/counts".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetCurrentUser;

impl Endpoint for GetCurrentUser {
    type Output = UserDto;
    fn path(&self) -> String {
        "/users/me".into()
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUserConfiguration {
    pub user_id: Uuid,
    pub config: UserConfiguration,
}

impl Endpoint for UpdateUserConfiguration {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/configuration", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.config).unwrap_or_default())
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetUsers;

impl Endpoint for GetUsers {
    type Output = Vec<UserDto>;
    fn path(&self) -> String {
        "/users".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateUser {
    pub name: String,
    pub password: String,
}

impl Endpoint for CreateUser {
    type Output = UserDto;
    fn path(&self) -> String {
        "/users/new".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::json!({ "Name": self.name, "Password": self.password }))
    }
}

#[derive(Debug, Clone)]
pub struct DeleteUser {
    pub user_id: Uuid,
}

impl Endpoint for DeleteUser {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}", self.user_id)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUser {
    pub user_id: Uuid,
    pub dto: UserDto,
}

impl Endpoint for UpdateUser {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.dto).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateUserPolicy {
    pub user_id: Uuid,
    pub policy: UserPolicy,
}

impl Endpoint for UpdateUserPolicy {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/policy", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.policy).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct AdminSetPassword {
    pub user_id: Uuid,
    pub new_pw: String,
}

impl Endpoint for AdminSetPassword {
    type Output = ();
    fn path(&self) -> String {
        format!("/users/{}/password", self.user_id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::json!({ "NewPw": self.new_pw }))
    }
}

impl Endpoint for AuthenticateUserByName {
    type Output = AuthenticateUserByNameResult;

    fn path(&self) -> String {
        "/users/authenticatebyname".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn body(&self) -> Body {
        Body::Json(serde_json::json!({
            "Username": self.username,
            "Pw": self.pw,
        }))
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetTunerHosts;

impl Endpoint for GetTunerHosts {
    type Output = Vec<TunerHostInfo>;
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
}

#[derive(Debug, Clone)]
pub struct AddTunerHost {
    pub info: TunerHostInfo,
}

impl Endpoint for AddTunerHost {
    type Output = TunerHostInfo;
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.info).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteTunerHost {
    pub id: String,
}

impl Endpoint for DeleteTunerHost {
    type Output = ();
    fn path(&self) -> String {
        "/livetv/tunerhosts".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("id".into(), self.id.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetEpgSources;

impl Endpoint for GetEpgSources {
    type Output = Vec<EpgSourceInfo>;
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
}

#[derive(Debug, Clone)]
pub struct SaveEpgSource {
    pub info: EpgSourceInfo,
}

impl Endpoint for SaveEpgSource {
    type Output = EpgSourceInfo;
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.info).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteEpgSource {
    pub id: String,
}

impl Endpoint for DeleteEpgSource {
    type Output = ();
    fn path(&self) -> String {
        "/remux/iptv/epgsources".into()
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("id".into(), self.id.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetIptvChannels {
    pub limit: u32,
    pub offset: u32,
    pub search: String,
    pub enabled: Option<bool>,
    pub country: String,
    pub group: String,
    pub sort: String,
}

impl Endpoint for GetIptvChannels {
    type Output = IptvChannelsResult;
    fn path(&self) -> String {
        "/remux/iptv/channels".into()
    }
    fn query(&self) -> Vec<(String, String)> {
        let mut q = vec![
            ("limit".into(), self.limit.to_string()),
            ("offset".into(), self.offset.to_string()),
        ];
        if !self.search.is_empty() {
            q.push(("search".into(), self.search.clone()));
        }
        if let Some(e) = self.enabled {
            q.push(("enabled".into(), e.to_string()));
        }
        if !self.country.is_empty() {
            q.push(("country".into(), self.country.clone()));
        }
        if !self.group.is_empty() {
            q.push(("group".into(), self.group.clone()));
        }
        if !self.sort.is_empty() {
            q.push(("sort".into(), self.sort.clone()));
        }
        q
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetIptvChannelCountries;

#[derive(Debug, Clone, Default)]
pub struct GetIptvChannelGroups;

impl Endpoint for GetIptvChannelCountries {
    type Output = Vec<String>;
    fn path(&self) -> String {
        "/remux/iptv/channels/countries".into()
    }
}

impl Endpoint for GetIptvChannelGroups {
    type Output = Vec<String>;
    fn path(&self) -> String {
        "/remux/iptv/channels/groups".into()
    }
}

#[derive(Debug, Clone)]
pub struct PatchChannel {
    pub id: String,
    pub patch: PatchChannelRequest,
}

impl Endpoint for PatchChannel {
    type Output = ();
    fn path(&self) -> String {
        format!("/remux/iptv/channels/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::PATCH
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.patch).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct BulkChannels {
    pub request: BulkChannelRequest,
}

impl Endpoint for BulkChannels {
    type Output = ();
    fn path(&self) -> String {
        "/remux/iptv/channels/bulk".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.request).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct AuthorizeQuickConnect {
    pub code: String,
}

impl Endpoint for AuthorizeQuickConnect {
    type Output = bool;

    fn path(&self) -> String {
        "/quickconnect/authorize".into()
    }

    fn method(&self) -> Method {
        Method::POST
    }

    fn query(&self) -> Vec<(String, String)> {
        vec![("Code".into(), self.code.clone())]
    }
}

#[derive(Debug, Clone, Default)]
pub struct GetApiKeys;

impl Endpoint for GetApiKeys {
    type Output = QueryResult<AuthenticationInfo>;
    fn path(&self) -> String {
        "/auth/keys".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateApiKey {
    pub app: String,
}

impl Endpoint for CreateApiKey {
    type Output = AuthenticationInfo;
    fn path(&self) -> String {
        "/auth/keys".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("app".into(), self.app.clone())]
    }
}

#[derive(Debug, Clone)]
pub struct DeleteApiKey {
    pub key: String,
}

impl Endpoint for DeleteApiKey {
    type Output = ();
    fn path(&self) -> String {
        format!("/auth/keys/{}", self.key)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

// --- Addons ---

#[derive(Debug, Clone, Default)]
pub struct ListAddonKinds;

impl Endpoint for ListAddonKinds {
    type Output = Vec<AddonMetadata>;
    fn path(&self) -> String {
        "/addon-kinds".into()
    }
}

#[derive(Debug, Clone, Default)]
pub struct ListAddons;

impl Endpoint for ListAddons {
    type Output = Vec<AddonDto>;
    fn path(&self) -> String {
        "/addons".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateAddon {
    pub payload: CreateAddonRequest,
}

impl Endpoint for CreateAddon {
    type Output = AddonDto;
    fn path(&self) -> String {
        "/addons".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateAddon {
    pub id: Uuid,
    pub payload: UpdateAddonRequest,
}

impl Endpoint for UpdateAddon {
    type Output = AddonDto;
    fn path(&self) -> String {
        format!("/addons/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteAddon {
    pub id: Uuid,
}

impl Endpoint for DeleteAddon {
    type Output = ();
    fn path(&self) -> String {
        format!("/addons/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

/// Fetch catalogs for an addon, auto-registering any new ones in the DB.
#[derive(Debug, Clone)]
pub struct GetAddonCatalogs {
    pub id: Uuid,
}

impl Endpoint for GetAddonCatalogs {
    type Output = Vec<AddonCatalogDto>;
    fn path(&self) -> String {
        format!("/addons/{}/catalogs", self.id)
    }
}

/// Batch-update enabled/max_items for an addon's catalogs.
#[derive(Debug, Clone)]
pub struct UpdateAddonCatalogs {
    pub id: Uuid,
    pub payload: Vec<UpdateAddonCatalogRequest>,
}

impl Endpoint for UpdateAddonCatalogs {
    type Output = ();
    fn path(&self) -> String {
        format!("/addons/{}/catalogs", self.id)
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

// ── Stream Groups ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamResolution {
    #[serde(rename = "2160p")]
    R2160p,
    #[serde(rename = "1080p")]
    R1080p,
    #[serde(rename = "720p")]
    R720p,
    #[serde(rename = "480p")]
    R480p,
    #[serde(rename = "360p")]
    R360p,
    Unknown,
}

impl StreamResolution {
    pub fn label(&self) -> &'static str {
        match self {
            Self::R2160p => "2160p",
            Self::R1080p => "1080p",
            Self::R720p => "720p",
            Self::R480p => "480p",
            Self::R360p => "360p",
            Self::Unknown => "Unknown",
        }
    }

    pub fn from_hunch(s: &str) -> Option<Self> {
        match s {
            "2160p" => Some(Self::R2160p),
            "1080p" => Some(Self::R1080p),
            "720p" => Some(Self::R720p),
            "480p" => Some(Self::R480p),
            "360p" => Some(Self::R360p),
            _ => None,
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::R2160p,
            Self::R1080p,
            Self::R720p,
            Self::R480p,
            Self::R360p,
            Self::Unknown,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamQuality {
    BluRayRemux,
    BluRay,
    WebDl,
    WebRip,
    Hdtv,
    Dvd,
    Tv,
    Unknown,
}

impl StreamQuality {
    pub fn label(&self) -> &'static str {
        match self {
            Self::BluRayRemux => "Blu-ray Remux",
            Self::BluRay => "Blu-ray",
            Self::WebDl => "WEB-DL",
            Self::WebRip => "WEBRip",
            Self::Hdtv => "HDTV",
            Self::Dvd => "DVD",
            Self::Tv => "TV",
            Self::Unknown => "Unknown",
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::BluRayRemux,
            Self::BluRay,
            Self::WebDl,
            Self::WebRip,
            Self::Hdtv,
            Self::Dvd,
            Self::Tv,
            Self::Unknown,
        ]
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamCodec {
    H264,
    H265,
    Vp9,
    Vc1,
    Mpeg2,
    Unknown,
}

impl StreamCodec {
    pub fn label(&self) -> &'static str {
        match self {
            Self::H264 => "H.264",
            Self::H265 => "H.265",
            Self::Vp9 => "VP9",
            Self::Vc1 => "VC-1",
            Self::Mpeg2 => "MPEG-2",
            Self::Unknown => "Unknown",
        }
    }

    pub fn from_hunch(s: &str) -> Option<Self> {
        match s {
            "H.264" => Some(Self::H264),
            "H.265" => Some(Self::H265),
            "VP9" => Some(Self::Vp9),
            "VC-1" => Some(Self::Vc1),
            "MPEG-2" => Some(Self::Mpeg2),
            _ => None,
        }
    }

    pub fn all() -> &'static [Self] {
        &[
            Self::H264,
            Self::H265,
            Self::Vp9,
            Self::Vc1,
            Self::Mpeg2,
            Self::Unknown,
        ]
    }
}

/// One condition in a stream group filter. Mirrors `FilterRule` but for stream attributes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "field", rename_all = "snake_case")]
pub enum StreamRule {
    Resolution {
        op: SetOp,
        values: Vec<StreamResolution>,
    },
    Quality {
        op: SetOp,
        values: Vec<StreamQuality>,
    },
    Codec {
        op: SetOp,
        values: Vec<StreamCodec>,
    },
}

/// Filter stored on a StreamGroup; analogous to CollectionFilter but evaluated
/// against hunch-parsed stream filenames rather than SQL fields.
#[derive(Default, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct StreamFilter {
    #[serde(default)]
    pub match_mode: FilterMatchMode,
    #[serde(default)]
    pub rules: Vec<StreamRule>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamGroupDto {
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub filter: StreamFilter,
    pub priority: i64,
    pub enabled: bool,
    #[serde(default)]
    pub hidden: bool,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateStreamGroupRequest {
    pub name: String,
    #[serde(default)]
    pub filter: StreamFilter,
    #[serde(default)]
    pub priority: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStreamGroupRequest {
    pub name: String,
    #[serde(default)]
    pub filter: StreamFilter,
    pub priority: i64,
    pub enabled: bool,
    #[serde(default)]
    pub hidden: bool,
}

#[derive(Debug, Clone, Default)]
pub struct ListStreamGroups;

impl Endpoint for ListStreamGroups {
    type Output = Vec<StreamGroupDto>;
    fn path(&self) -> String {
        "/remux/stream-groups".into()
    }
}

#[derive(Debug, Clone)]
pub struct CreateStreamGroup {
    pub payload: CreateStreamGroupRequest,
}

impl Endpoint for CreateStreamGroup {
    type Output = StreamGroupDto;
    fn path(&self) -> String {
        "/remux/stream-groups".into()
    }
    fn method(&self) -> Method {
        Method::POST
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct UpdateStreamGroup {
    pub id: Uuid,
    pub payload: UpdateStreamGroupRequest,
}

impl Endpoint for UpdateStreamGroup {
    type Output = StreamGroupDto;
    fn path(&self) -> String {
        format!("/remux/stream-groups/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::PUT
    }
    fn body(&self) -> Body {
        Body::Json(serde_json::to_value(&self.payload).unwrap_or_default())
    }
}

#[derive(Debug, Clone)]
pub struct DeleteStreamGroup {
    pub id: Uuid,
}

impl Endpoint for DeleteStreamGroup {
    type Output = ();
    fn path(&self) -> String {
        format!("/remux/stream-groups/{}", self.id)
    }
    fn method(&self) -> Method {
        Method::DELETE
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamGroupPreviewGroupDto {
    pub name: String,
    pub hidden: bool,
    pub streams: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreamGroupPreviewDto {
    pub groups: Vec<StreamGroupPreviewGroupDto>,
    pub ungrouped: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct GetStreamGroupPreview {
    pub imdb_id: String,
}

impl Endpoint for GetStreamGroupPreview {
    type Output = StreamGroupPreviewDto;
    fn path(&self) -> String {
        "/remux/stream-groups/preview".into()
    }
    fn query(&self) -> Vec<(String, String)> {
        vec![("imdb_id".into(), self.imdb_id.clone())]
    }
}

// ---------------------------------------------------------------------------
// Item refresh
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
pub enum MetadataRefreshMode {
    #[default]
    None,
    ValidationOnly,
    Default,
    #[serde(other)]
    FullRefresh,
}

#[derive(Debug, Deserialize, Default, PartialEq, Eq)]
pub enum ImageRefreshMode {
    #[default]
    None,
    ValidationOnly,
    Default,
    #[serde(other)]
    FullRefresh,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
pub struct RefreshItemQuery {
    #[serde(default)]
    pub metadata_refresh_mode: MetadataRefreshMode,
    #[serde(default)]
    pub image_refresh_mode: ImageRefreshMode,
    #[serde(default)]
    pub replace_all_metadata: bool,
    #[serde(default)]
    pub replace_all_images: bool,
    #[serde(default)]
    pub recursive: bool,
    #[serde(default)]
    pub regenerate_trickplay: bool,
}
