use anyhow::Result;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous,
};
use std::str::FromStr;
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

pub async fn connect(url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal);
    Ok(SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(opts)
        .await?)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    // Must run before SQL migrations so media_id column is still available.
    migrate_user_state_keys(pool).await?;
    sqlx::migrate!("./migrations").run(pool).await?;
    backfill_certification_age(pool).await?;
    Ok(())
}

/// Migrates `user_media_state.media_key` from old `media.media_id` strings to
/// UUID simple strings (e.g., "tt1234567" → "a1b2c3d4...").
///
/// Only runs when the `media_id` column still exists (i.e., before the SQL
/// migration that drops it). Safe to call on already-migrated databases.
async fn migrate_user_state_keys(pool: &SqlitePool) -> Result<()> {
    let has_media_id: bool = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM pragma_table_info('media') WHERE name = 'media_id'",
    )
    .fetch_one(pool)
    .await
    .map(|n| n > 0)
    .unwrap_or(false);

    if !has_media_id {
        return Ok(());
    }

    // Find user_media_state rows whose media_key matches a media.media_id,
    // and update them to use the media's UUID simple string instead.
    let to_update: Vec<(String, uuid::Uuid)> = sqlx::query_as(
        "SELECT ums.media_key, m.id
         FROM user_media_state ums
         JOIN media m ON m.media_id = ums.media_key",
    )
    .fetch_all(pool)
    .await?;

    for (old_key, new_id) in to_update {
        let new_key = new_id.as_simple().to_string();
        if old_key == new_key {
            continue;
        }
        sqlx::query("UPDATE user_media_state SET media_key = ?1 WHERE media_key = ?2")
            .bind(&new_key)
            .bind(&old_key)
            .execute(pool)
            .await?;
    }

    Ok(())
}

async fn backfill_certification_age(pool: &SqlitePool) -> Result<()> {
    let config = Settings::get_config(pool).await.unwrap_or_default();
    let rows = sqlx::query_as::<_, (uuid::Uuid, String)>(
        "SELECT id, certification FROM media WHERE certification IS NOT NULL AND certification_age IS NULL",
    )
    .fetch_all(pool)
    .await?;

    for (id, certification) in rows {
        if let Some(age) = crate::localization::ratings::resolve_rating_age(
            Some(&certification),
            config.metadata_country_code.as_deref(),
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
