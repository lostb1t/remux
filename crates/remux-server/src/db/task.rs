use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::common::get_uuid;
use remux_sdks::remux::models::TaskTriggerInfoType;

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskTrigger {
    pub id: String,
    pub task_id: String,
    #[sqlx(try_from = "String")]
    pub kind: TaskTriggerInfoType,
    pub time_limit_hours: Option<i64>,
    pub cron: Option<String>,
}

impl Default for TaskTrigger {
    fn default() -> Self {
        Self {
            id: get_uuid().to_string(),
            task_id: String::new(),
            kind: TaskTriggerInfoType::DailyTrigger,
            time_limit_hours: None,
            cron: None,
        }
    }
}

impl TaskTrigger {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO task_triggers (
                id,
                task_id,
                kind,
                time_limit_hours,
                cron
            )
            VALUES (?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                task_id          = excluded.task_id,
                kind             = excluded.kind,
                time_limit_hours = excluded.time_limit_hours,
                cron             = excluded.cron
            "#,
        )
        .bind(&self.id)
        .bind(&self.task_id)
        .bind(self.kind.to_string())
        .bind(self.time_limit_hours)
        .bind(&self.cron)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(r#"SELECT * FROM task_triggers"#)
            .fetch_all(db)
            .await?)
    }

    pub async fn get_by_task_id(db: &SqlitePool, task_id: &str) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            r#"SELECT * FROM task_triggers WHERE LOWER(task_id) = LOWER(?1)"#,
        )
        .bind(task_id)
        .fetch_all(db)
        .await?)
    }

    pub async fn delete_by_task_id(db: &SqlitePool, task_id: &str) -> Result<()> {
        sqlx::query(r#"DELETE FROM task_triggers WHERE LOWER(task_id) = LOWER(?1)"#)
            .bind(task_id)
            .execute(db)
            .await?;
        Ok(())
    }
}

#[derive(
    Debug,
    Clone,
    PartialEq,
    Serialize,
    Deserialize,
    sqlx::Type,
    strum_macros::EnumString,
    strum_macros::Display,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum TaskResultStatus {
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskResult {
    pub task_id: String, // Now uses task key instead of UUID
    pub start_at: NaiveDateTime,
    pub end_at: NaiveDateTime,
    pub status: TaskResultStatus,
}

impl TaskResult {
    pub fn new(task_id: &str, status: TaskResultStatus) -> Self {
        let now = Utc::now().naive_utc();
        Self {
            task_id: task_id.to_string(),
            start_at: now,
            end_at: now,
            status,
        }
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO task_results (task_id, start_at, end_at, status)
            VALUES (?, ?, ?, ?)
            ON CONFLICT(task_id) DO UPDATE SET
                start_at = excluded.start_at,
                end_at   = excluded.end_at,
                status    = excluded.status
            "#,
        )
        .bind(&self.task_id)
        .bind(self.start_at)
        .bind(self.end_at)
        .bind(&self.status)
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_by_task_id(
        db: &SqlitePool,
        task_id: &str,
    ) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            r#"
            SELECT *
            FROM task_results
            WHERE task_id = ?1
            ORDER BY end_at DESC
            LIMIT 1
            "#,
        )
        .bind(task_id)
        .fetch_optional(db)
        .await?)
    }
}
