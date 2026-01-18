use anyhow::Result;
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use std::str::FromStr;

pub mod auth;
pub mod media;
pub mod user;
pub use media::*;
pub use user::*;

pub async fn connect(url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(url)?;
    Ok(SqlitePoolOptions::new()
        .max_connections(10)
        .connect_with(opts)
        .await?)
}

pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::migrate!("./migrations").run(pool).await?;
    Ok(())
}

pub async fn checkpoint_db(pool: &SqlitePool) {
    sqlx::query("PRAGMA wal_checkpoint(FULL)")
        .execute(pool)
        .await;
}

pub struct FilterResult<T> {
    pub records: Vec<T>,
    pub total_count: usize,
}