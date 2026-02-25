use anyhow::Result;
use sqlx::SqlitePool;

pub struct Settings;

impl Settings {
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
