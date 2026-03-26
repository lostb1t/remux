use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct ApiKey {
    pub access_token: String,
    pub app_name: String,
    pub created_at: DateTime<Utc>,
}

impl ApiKey {
    pub async fn create(db: &SqlitePool, app_name: &str) -> Result<Self> {
        let token = uuid::Uuid::new_v4().to_string().replace('-', "");
        sqlx::query("INSERT INTO api_keys (access_token, app_name) VALUES (?1, ?2)")
            .bind(&token)
            .bind(app_name)
            .execute(db)
            .await?;
        Ok(Self::get_by_token(db, &token).await?.unwrap())
    }

    pub async fn get_by_token(db: &SqlitePool, token: &str) -> Result<Option<Self>> {
        Ok(
            sqlx::query_as::<_, Self>("SELECT * FROM api_keys WHERE access_token = ?1")
                .bind(token)
                .fetch_optional(db)
                .await?,
        )
    }

    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(
            sqlx::query_as::<_, Self>(
                "SELECT * FROM api_keys ORDER BY created_at DESC",
            )
            .fetch_all(db)
            .await?,
        )
    }

    pub async fn delete(db: &SqlitePool, token: &str) -> Result<()> {
        sqlx::query("DELETE FROM api_keys WHERE access_token = ?1")
            .bind(token)
            .execute(db)
            .await?;
        Ok(())
    }
}
