//! Shared DTOs for the addon API. Used by server (in `crates/remux-server`)
//! and dashboard (in `crates/remux-dashboard`). The runtime traits and
//! registry live in `remux_server::addons`.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_with::skip_serializing_none;
use uuid::Uuid;

/// Resources an addon can serve. Mirrors Stremio's `ResourceType` plus
/// our additions (`Search`, `Lyrics`).
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "lowercase")]
#[strum(serialize_all = "lowercase")]
pub enum AddonResource {
    Catalog,
    Meta,
    Search,
    Subtitles,
    Streams,
    Lyrics,
    Segment,
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

/// Static metadata describing one kind of addon. Returned by `GET /addon-kinds`
/// so the dashboard can populate the kind picker and config form.
#[skip_serializing_none]
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddonKindMetadata {
    pub id: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub supported_resources: Vec<AddonResource>,
    pub supported_types: Vec<String>,
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
    pub resources: Vec<AddonResource>,
    /// All resources the addon actually provides. For Stremio addons this is
    /// populated from the manifest; for other kinds it mirrors the static kind
    /// metadata. Used by the dashboard as the checkbox option list.
    #[serde(default)]
    pub supported_resources: Vec<AddonResource>,
    /// Content types the addon supports (e.g. `"movie"`, `"series"`). For
    /// Stremio addons this comes from the manifest; for others from kind
    /// metadata.
    #[serde(default)]
    pub supported_types: Vec<String>,
    pub priority: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Create payload — `POST /addons`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAddonRequest {
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub config: serde_json::Value,
    #[serde(default)]
    pub resources: Vec<AddonResource>,
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
    pub resources: Option<Vec<AddonResource>>,
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
