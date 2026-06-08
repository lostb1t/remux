use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::{
    ConnectOptions as _, SqlitePool,
    sqlite::{
        SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
    },
};
use std::{str::FromStr, time::Duration};
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
        .pragma("cache_size", "-65536")
        .pragma("mmap_size", "268435456")
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

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    // ignore_missing: migration 202506080001 was applied under the wrong timestamp
    // and renamed to 202606080001; ignore the now-orphaned DB record on existing installs.
    let mut migrator = sqlx::migrate!("./migrations");
    migrator.set_ignore_missing(true);
    migrator
        .run(pool)
        .await?;
    vacuum_if_needed(pool).await?;
    Ok(())
}

async fn vacuum_if_needed(pool: &SqlitePool) -> Result<()> {
    let freelist: i64 = sqlx::query_scalar("PRAGMA freelist_count")
        .fetch_one(pool)
        .await
        .unwrap_or(0);
    if freelist > 100 {
        tracing::info!(
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
