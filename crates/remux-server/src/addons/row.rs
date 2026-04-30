use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::collections::HashMap;
use uuid::Uuid;

use super::AddonResource;

/// Per-catalog state stored in `AddonRow.config["catalogs"]`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CatalogState {
    pub enabled: bool,
    pub max_items: Option<i64>,
}

/// One row of the `addons` table — config-time data only. Runtime objects
/// (`Addon`) are constructed from this via `AddonKind::instantiate`.
#[derive(Debug, Clone)]
pub struct AddonRow {
    pub id: Uuid,
    pub kind: String,
    pub name: String,
    pub config: serde_json::Value,
    pub resources: Vec<AddonResource>,
    pub priority: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl AddonRow {
    pub async fn list(db: &SqlitePool) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, RawAddonRow>(
            "SELECT id, kind, name, config, resources, priority, created_at, updated_at \
             FROM addons ORDER BY priority ASC, created_at ASC",
        )
        .fetch_all(db)
        .await?;
        rows.into_iter().map(|r| r.try_into()).collect()
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, RawAddonRow>(
            "SELECT id, kind, name, config, resources, priority, created_at, updated_at \
             FROM addons WHERE id = ?1",
        )
        .bind(id.to_string())
        .fetch_optional(db)
        .await?;
        row.map(Self::try_from).transpose()
    }

    pub async fn insert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(self.id.to_string())
        .bind(&self.kind)
        .bind(&self.name)
        .bind(serde_json::to_string(&self.config)?)
        .bind(serde_json::to_string(&self.resources)?)
        .bind(self.priority)
        .bind(self.created_at.to_rfc3339())
        .bind(self.updated_at.to_rfc3339())
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn update(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "UPDATE addons SET name = ?2, config = ?3, resources = ?4, priority = ?5, updated_at = ?6 \
             WHERE id = ?1",
        )
        .bind(self.id.to_string())
        .bind(&self.name)
        .bind(serde_json::to_string(&self.config)?)
        .bind(serde_json::to_string(&self.resources)?)
        .bind(self.priority)
        .bind(self.updated_at.to_rfc3339())
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM addons WHERE id = ?1")
            .bind(id.to_string())
            .execute(db)
            .await?;
        Ok(())
    }

    /// Returns the per-catalog states stored in `config["catalogs"]`.
    pub fn catalog_states(&self) -> HashMap<String, CatalogState> {
        self.config
            .get("catalogs")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default()
    }

    /// Overwrites `config["catalogs"]` with `states` and updates `updated_at`.
    pub fn set_catalog_states(&mut self, states: HashMap<String, CatalogState>) {
        self.config["catalogs"] = serde_json::to_value(states).unwrap_or_default();
        self.updated_at = Utc::now();
    }
}

#[derive(sqlx::FromRow)]
struct RawAddonRow {
    id: String,
    kind: String,
    name: String,
    config: String,
    resources: String,
    priority: i64,
    created_at: String,
    updated_at: String,
}

impl TryFrom<RawAddonRow> for AddonRow {
    type Error = anyhow::Error;

    fn try_from(r: RawAddonRow) -> Result<Self> {
        Ok(Self {
            id: Uuid::parse_str(&r.id)?,
            kind: r.kind,
            name: r.name,
            config: serde_json::from_str(&r.config)?,
            resources: serde_json::from_str(&r.resources)?,
            priority: r.priority,
            created_at: DateTime::parse_from_rfc3339(&r.created_at)?
                .with_timezone(&Utc),
            updated_at: DateTime::parse_from_rfc3339(&r.updated_at)?
                .with_timezone(&Utc),
        })
    }
}
