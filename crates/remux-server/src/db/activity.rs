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

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000).await.unwrap();
        crate::db::migrate(&db).await.unwrap();
        db
    }

    #[tokio::test]
    async fn insert_and_list() {
        let db = test_db().await;
        let uid = Uuid::new_v4();

        ActivityLog::insert(&db, &uid, "alice", "session_revoked", None, None, None, None, None)
            .await
            .unwrap();
        ActivityLog::insert(&db, &uid, "alice", "password_changed", None, None, None, None, None)
            .await
            .unwrap();

        let (rows, total) = ActivityLog::list(&db, 0, 50).await.unwrap();
        assert_eq!(total, 2);
        assert_eq!(rows.len(), 2);
        let actions: Vec<&str> = rows.iter().map(|r| r.action.as_str()).collect();
        assert!(actions.contains(&"session_revoked"));
        assert!(actions.contains(&"password_changed"));
    }

    #[tokio::test]
    async fn list_pagination() {
        let db = test_db().await;
        let uid = Uuid::new_v4();

        for i in 0..5 {
            ActivityLog::insert(&db, &uid, "alice", &format!("action_{i}"), None, None, None, None, None)
                .await
                .unwrap();
        }

        let (page1, total) = ActivityLog::list(&db, 0, 2).await.unwrap();
        assert_eq!(total, 5);
        assert_eq!(page1.len(), 2);

        let (page2, _) = ActivityLog::list(&db, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);
    }

    #[tokio::test]
    async fn insert_with_target_fields() {
        let db = test_db().await;
        let actor = Uuid::new_v4();
        let target = Uuid::new_v4();

        ActivityLog::insert(
            &db, &actor, "admin", "session_revoked",
            Some(&target), Some("bob"), Some("dev-id"), Some("Bob's phone"), Some("forced"),
        )
        .await
        .unwrap();

        let (rows, _) = ActivityLog::list(&db, 0, 10).await.unwrap();
        let row = &rows[0];
        assert_eq!(row.target_user_name.as_deref(), Some("bob"));
        assert_eq!(row.device_name.as_deref(), Some("Bob's phone"));
        assert_eq!(row.details.as_deref(), Some("forced"));
    }
}
