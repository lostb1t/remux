use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{
    ConnectOptions as _, SqlitePool,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    },
};
use std::{str::FromStr, time::Duration};
use tracing::{info, warn};
pub mod api_key;
pub mod auth;
pub mod image;
pub mod iptv;
pub mod media;
pub mod settings;
pub mod stream_group;
pub mod task;
pub mod user;
pub use api_key::*;
pub use image::*;
pub use iptv::*;
pub use media::*;
pub use settings::*;
pub use stream_group::*;
pub use task::*;
pub use user::*;

pub async fn connect(url: &str, slow_query_threshold_ms: u64) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .pragma("wal_autocheckpoint", "1000")
        .pragma("auto_vacuum", "incremental")
        .pragma("cache_size", "-16384")
        .pragma("mmap_size", "33554432")
        .pragma("temp_store", "memory")
        // Allow up to 10s of retrying when blocked by another connection's
        // write lock. This is what makes wal_checkpoint(TRUNCATE) actually
        // wait for in-flight reads to finish instead of giving up immediately.
        .busy_timeout(Duration::from_secs(10))
        .log_slow_statements(
            log::LevelFilter::Warn,
            Duration::from_millis(slow_query_threshold_ms),
        );
    Ok(SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?)
}

const MIN_SQUASHABLE: i64 = 202606140001; // minimum version (v0.7.0) that can upgrade via squash
const LAST_PRE_SQUASH: i64 = 202606140004; // last migration on main before this PR
const SQUASH_VERSION: i64 = 202606140005; // squash migration version

async fn prepare_squash(pool: &SqlitePool) -> Result<()> {
    let last: Option<i64> = sqlx::query_scalar(
        "SELECT version FROM _sqlx_migrations \
         WHERE success = TRUE ORDER BY version DESC LIMIT 1",
    )
    .fetch_optional(pool)
    .await
    .ok()
    .flatten();

    match last {
        None => {}
        Some(v) if v >= SQUASH_VERSION => {}
        Some(v) if v >= MIN_SQUASHABLE => {
            // Any DB from v0.7.0 through current main (versions 001–004) — clear and let squash run.
            sqlx::query("DELETE FROM _sqlx_migrations")
                .execute(pool)
                .await?;
        }
        Some(_) => anyhow::bail!(
            "Database schema is outdated. Please update to remux v0.8.0 first, \
             then upgrade to this version."
        ),
    }
    Ok(())
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    prepare_squash(pool).await?;
    sqlx::migrate!("./migrations")
        .run(pool)
        .await?;
    migrate_catalog_collections(pool).await;
    vacuum_if_needed(pool).await?;
    Ok(())
}

/// One-time migration: convert old manual collections (created by the removed
/// `create_collection` addon feature) into smart collections with a catalog
/// filter rule and CatalogOrder as the default sort.
///
/// Idempotent — the UPDATE is guarded by `collection_kind = 'manual'`, so it
/// only fires once per collection and is a no-op on subsequent startups.
async fn migrate_catalog_collections(pool: &SqlitePool) {
    use std::collections::HashMap;
    use uuid::Uuid;

    // Read addon IDs and their raw preset JSON to extract catalog local_ids.
    let rows: Vec<(Uuid, String)> =
        match sqlx::query_as("SELECT id, preset FROM addons")
            .fetch_all(pool)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "migrate_catalog_collections: failed to list addons");
                return;
            }
        };

    for (addon_id, preset_json) in rows {
        let catalog_keys: Vec<String> =
            serde_json::from_str::<serde_json::Value>(&preset_json)
                .ok()
                .and_then(|v| {
                    v.get("config")
                        .and_then(|c| c.get("catalogs"))
                        .and_then(|c| c.as_object())
                        .map(|m| {
                            m.keys()
                                .cloned()
                                .collect()
                        })
                })
                .unwrap_or_default();

        for local_id in catalog_keys {
            let old_manual_id = Uuid::new_v5(&addon_id, local_id.as_bytes());

            let collection_source = format!("{}:{}", addon_id, local_id);
            let collection_id: Uuid = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM media WHERE collection_kind = 'catalog' AND collection_source = ?",
            )
            .bind(&collection_source)
            .fetch_optional(pool)
            .await
            .unwrap_or(None)
            .unwrap_or(old_manual_id);

            let smart_filter = serde_json::json!({
                "match_mode": "all",
                "rules": [{"field": "catalog", "catalog_id": collection_id}]
            });

            let migrated = sqlx::query(
                "UPDATE media SET \
                 collection_kind = 'smart', \
                 collection_smart_filter = ?, \
                 collection_default_sort = COALESCE(collection_default_sort, '[\"CatalogOrder\"]'), \
                 collection_default_sort_order = COALESCE(collection_default_sort_order, '[\"Ascending\"]') \
                 WHERE id = ? AND collection_kind = 'manual'",
            )
            .bind(serde_json::to_string(&smart_filter).unwrap_or_default())
            .bind(old_manual_id)
            .execute(pool)
            .await
            .map(|r| r.rows_affected())
            .unwrap_or(0);

            if migrated > 0 {
                info!(
                    addon = %addon_id,
                    catalog = %local_id,
                    collection_id = %collection_id,
                    "migrated manual collection to smart catalog collection"
                );
                sqlx::query(
                    "DELETE FROM media_relations WHERE left_media_id = ? AND role = 'collection'",
                )
                .bind(old_manual_id)
                .execute(pool)
                .await
                .ok();
            }
        }
    }
}

async fn vacuum_if_needed(pool: &SqlitePool) -> Result<()> {
    let freelist: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    if freelist > 100 {
        info!(
            freelist_pages = freelist,
            "vacuuming database to apply auto_vacuum mode and reclaim freed pages"
        );
        sqlx::query("VACUUM")
            .execute(pool)
            .await?;
    }
    Ok(())
}

async fn backfill_certification_age(pool: &SqlitePool) -> Result<()> {
    let config = Settings::get_config(pool)
        .await
        .unwrap_or_default();
    let rows = sqlx::query_as::<_, (uuid::Uuid, String)>(
        "SELECT id, certification FROM media WHERE certification IS NOT NULL AND certification_age IS NULL",
    )
    .fetch_all(pool)
    .await?;

    for (id, certification) in rows {
        if let Some(age) = crate::localization::ratings::resolve_rating_age(
            Some(&certification),
            config
                .metadata_country_code
                .as_deref(),
        ) {
            sqlx::query("UPDATE media SET certification_age = ?1 WHERE id = ?2")
                .bind(age)
                .bind(id)
                .execute(pool)
                .await?;
        }
    }

    Ok(())
}

pub async fn checkpoint_db(pool: &SqlitePool) {
    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(pool)
        .await;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum ScrollDirection {
    Horizontal,
    Vertical,
}

pub struct FilterResult<T> {
    pub records: Vec<T>,
    pub total_count: usize,
}

trait QueryBuilderExt<'q> {
    fn push_in<T>(&mut self, column: &str, values: &'q Vec<T>)
    where
        T: Send
            + Sync
            + for<'a> sqlx::Encode<'a, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>
            + 'q;
}

impl<'q> QueryBuilderExt<'q> for sqlx::QueryBuilder<'q, sqlx::Sqlite> {
    fn push_in<T>(&mut self, column: &str, values: &'q Vec<T>)
    where
        T: Send
            + Sync
            + for<'a> sqlx::Encode<'a, sqlx::Sqlite>
            + sqlx::Type<sqlx::Sqlite>
            + 'q,
    {
        if values.is_empty() {
            return;
        };

        self.push(" AND ");
        self.push(column);
        self.push(" IN (");

        let mut separated = self.separated(", ");
        for v in values {
            separated.push_bind(v);
        }

        self.push(")");
    }
}
