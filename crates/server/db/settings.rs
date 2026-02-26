use anyhow::Result;
use sqlx::SqlitePool;

use crate::jellyfin::ServerConfiguration;

const SERVER_CONFIG_KEY: &str = "server_configuration";

pub struct Settings;

impl Settings {
    pub async fn get_config(db: &SqlitePool) -> Result<ServerConfiguration> {
        Ok(match Self::get(db, SERVER_CONFIG_KEY).await? {
            Some(json) => serde_json::from_str(&json).unwrap_or_default(),
            None => ServerConfiguration::default(),
        })
    }

    pub async fn set_config(db: &SqlitePool, config: &ServerConfiguration) -> Result<()> {
        let json = serde_json::to_string(config)?;
        Self::set(db, SERVER_CONFIG_KEY, &json).await
    }

    pub async fn get(db: &SqlitePool, key: &str) -> Result<Option<String>> {
        let row = sqlx::query_scalar::<_, String>(
            "SELECT value FROM settings WHERE key = ?1",
        )
        .bind(key)
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    pub async fn set(db: &SqlitePool, key: &str, value: &str) -> Result<()> {
        sqlx::query(
            "INSERT INTO settings (key, value) VALUES (?1, ?2)
             ON CONFLICT (key) DO UPDATE SET value = excluded.value",
        )
        .bind(key)
        .bind(value)
        .execute(db)
        .await?;
        Ok(())
    }
}
