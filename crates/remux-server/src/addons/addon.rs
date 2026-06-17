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
    pub resources: Vec<remux_sdks::stremio::ResourceRef>,
    /// Content types the user has enabled for this addon (e.g. `"movie"`, `"series"`).
    /// Empty means all types are enabled.
    #[sqlx(json)]
    pub types: Vec<MediaKind>,
    pub enabled: bool,
    pub priority: i64,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

const ADDON_COLS: &str =
    "id, name, preset, resources, types, enabled, priority, created_at, updated_at";

impl Addon {
    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {ADDON_COLS} FROM addons ORDER BY priority ASC, created_at ASC"
        ))
        .fetch_all(db)
        .await?)
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {ADDON_COLS} FROM addons WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(db)
        .await?)
    }

    pub async fn insert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO addons \
             (id, name, preset, resources, types, enabled, priority, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
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
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn update(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "UPDATE addons \
             SET name = ?2, preset = ?3, resources = ?4, types = ?5, \
                 enabled = ?6, priority = ?7, updated_at = ?8 \
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

    pub fn has_resource(&self, kind: &remux_sdks::stremio::ResourceType) -> bool {
        self.resources
            .iter()
            .any(|r| &r.name == kind)
    }

    pub fn resource_id_prefixes(
        &self,
        kind: &remux_sdks::stremio::ResourceType,
    ) -> Option<&[String]> {
        self.resources
            .iter()
            .find(|r| &r.name == kind)
            .and_then(|r| {
                r.id_prefixes
                    .as_deref()
            })
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
