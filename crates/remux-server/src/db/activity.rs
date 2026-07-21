use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(rename_all = "PascalCase")]
pub struct ActivityLog {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub user_id: String,
    pub user_name: String,
    pub action: String,
    pub target_user_id: Option<String>,
    pub target_user_name: Option<String>,
    pub device_id: Option<String>,
    pub device_name: Option<String>,
    pub details: Option<String>,
}

impl ActivityLog {
    pub async fn insert(
        db: &SqlitePool,
        user_id: &Uuid,
        user_name: &str,
        action: &str,
        target_user_id: Option<&Uuid>,
        target_user_name: Option<&str>,
        device_id: Option<&str>,
        device_name: Option<&str>,
        details: Option<&str>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO activity_log (id, user_id, user_name, action, target_user_id, target_user_name, device_id, device_name, details) \
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(Uuid::new_v4().to_string())
        .bind(user_id.to_string())
        .bind(user_name)
        .bind(action)
        .bind(target_user_id.map(|u| u.to_string()))
        .bind(target_user_name)
        .bind(device_id)
        .bind(device_name)
        .bind(details)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn list(db: &SqlitePool, start_index: i64, limit: i64) -> Result<(Vec<Self>, i64)> {
        let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM activity_log")
            .fetch_one(db)
            .await?;

        let rows = sqlx::query_as::<_, Self>(
            "SELECT * FROM activity_log ORDER BY timestamp DESC LIMIT ? OFFSET ?",
        )
        .bind(limit)
        .bind(start_index)
        .fetch_all(db)
        .await?;

        Ok((rows, total))
    }
}
