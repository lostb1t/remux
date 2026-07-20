use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use super::AddonPresetRef;
use crate::db::MediaKind;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogState {
    pub enabled: bool,
    pub max_items: Option<i64>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Addon {
    pub id: Uuid,
    pub name: String,
    #[sqlx(json)]
    pub preset: AddonPresetRef,
    #[sqlx(json)]
    pub resources: Vec<remux_sdks::stremio::ResourceType>,
    /// Content types the user has enabled for this addon (e.g. `"movie"`, `"series"`).
    /// Empty means all types are enabled.
    #[sqlx(json)]
    pub types: Vec<MediaKind>,
    pub enabled: bool,
    pub priority: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    /// System addons cannot be deleted or have their resources/content types modified.
    pub system: bool,
    /// Included in the default addon list (users with no override see this addon).
    /// Set to false for per-user-only addons that must be explicitly assigned.
    pub is_default: bool,
}

const ADDON_COLS: &str = "id, name, preset, resources, types, enabled, priority, created_at, updated_at, system, is_default";

impl Addon {
    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        let addons = sqlx::query_as::<_, Self>(&format!(
            "SELECT {ADDON_COLS} FROM addons ORDER BY priority ASC, created_at ASC"
        ))
        .fetch_all(db)
        .await?;
        Ok(addons)
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        let addon = sqlx::query_as::<_, Self>(&format!(
            "SELECT {ADDON_COLS} FROM addons WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(db)
        .await?;
        Ok(addon)
    }

    pub async fn insert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO addons \
             (id, name, preset, resources, types, enabled, priority, created_at, updated_at, system, is_default) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(self.id)
        .bind(&self.name)
        .bind(sqlx::types::Json(&self.preset))
        .bind(sqlx::types::Json(&self.resources))
        .bind(sqlx::types::Json(&self.types))
        .bind(self.enabled)
        .bind(self.priority)
        .bind(self.created_at)
        .bind(self.updated_at)
        .bind(self.system)
        .bind(self.is_default)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn update(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "UPDATE addons \
             SET name = ?2, preset = ?3, resources = ?4, types = ?5, \
                 enabled = ?6, priority = ?7, updated_at = ?8, is_default = ?9 \
             WHERE id = ?1",
        )
        .bind(self.id)
        .bind(&self.name)
        .bind(sqlx::types::Json(&self.preset))
        .bind(sqlx::types::Json(&self.resources))
        .bind(sqlx::types::Json(&self.types))
        .bind(self.enabled)
        .bind(self.priority)
        .bind(self.updated_at)
        .bind(self.is_default)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM addons WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }

    pub fn catalog_states(&self) -> HashMap<String, CatalogState> {
        self.preset
            .config
            .get("catalogs")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    pub fn set_catalog_states(&mut self, states: HashMap<String, CatalogState>) {
        self.preset
            .config["catalogs"] = serde_json::to_value(states).unwrap_or_default();
        self.updated_at = Utc::now().naive_utc();
    }
}

/// Returns the ordered list of addon IDs for a user's override, or `None` if the user
/// has no override (meaning they use the full default addon list).
pub async fn user_addon_override(
    db: &SqlitePool,
    user_id: Uuid,
) -> Result<Option<Vec<Uuid>>> {
    let ids: Vec<Uuid> = sqlx::query_scalar(
        "SELECT addon_id FROM addon_users WHERE user_id = ?1 ORDER BY priority ASC",
    )
    .bind(user_id)
    .fetch_all(db)
    .await?;
    Ok(if ids.is_empty() { None } else { Some(ids) })
}

/// Replace the addon override list for a user.
/// Passing an empty slice removes the override (user falls back to the default list).
pub async fn set_user_addon_override(
    db: &SqlitePool,
    user_id: Uuid,
    addon_ids: &[Uuid],
) -> Result<()> {
    let mut tx = db
        .begin()
        .await?;
    sqlx::query("DELETE FROM addon_users WHERE user_id = ?1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for (priority, addon_id) in addon_ids
        .iter()
        .enumerate()
    {
        sqlx::query(
            "INSERT OR IGNORE INTO addon_users (addon_id, user_id, priority) VALUES (?1, ?2, ?3)",
        )
        .bind(addon_id)
        .bind(user_id)
        .bind(priority as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit()
        .await?;
    Ok(())
}
