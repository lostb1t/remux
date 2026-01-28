use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::utils::get_uuid;

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
pub enum TaskTriggerKind {
    Schedule,
    Startup,
}

impl TryFrom<String> for TaskTriggerKind {
    type Error = strum::ParseError;

    fn try_from(s: String) -> Result<Self, Self::Error> {
        Self::try_from(s.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskTrigger {
    pub id: Uuid,
    pub task_id: Uuid,
    pub kind: TaskTriggerKind,
    pub time_limit_hours: Option<i64>,
    pub cron: Option<String>,
}

impl Default for TaskTrigger {
    fn default() -> Self {
        Self {
            id: get_uuid(),
            task_id: Uuid::nil(),
            kind: TaskTriggerKind::Schedule,
            time_limit_hours: None,
            cron: None,
        }
    }
}

impl TaskTrigger {
    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        let id = self.id.to_string();
        let task_id = self.task_id.to_string();

        sqlx::query!(
            r#"
            INSERT INTO task_triggers (
                id,
                task_id,
                kind,
                time_limit_hours,
                cron
            )
            VALUES (?1, ?2, ?3, ?4, ?5)
            ON CONFLICT(id) DO UPDATE SET
                task_id          = excluded.task_id,
                kind             = excluded.kind,
                time_limit_hours = excluded.time_limit_hours,
                cron             = excluded.cron
            "#,
            id,
            task_id,
            self.kind,
            self.time_limit_hours,
            self.cron,
        )
        .execute(db)
        .await?;

        Ok(())
    }

    pub async fn get_all(db: &SqlitePool) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(
            r#"
            SELECT
            *
            FROM task_triggers
            "#,
        )
        .fetch_all(db)
        .await?)
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
pub enum TaskResultState {
    Completed,
    Failed,
    Stopped,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TaskResult {
    pub task_id: Uuid,
    pub start_at: NaiveDateTime,
    pub end_at: NaiveDateTime,
    pub state: TaskResultState,
}

impl TaskResult {
    pub fn new(task_id: Uuid, state: TaskResultState) -> Self {
        let now = Utc::now().naive_utc();
        Self {
            task_id,
            start_at: now,
            end_at: now,
            state,
        }
    }

    pub async fn save(&self, db: &SqlitePool) -> Result<()> {
        let task_id = self.task_id.to_string();

        sqlx::query!(
            r#"
            INSERT INTO task_results (task_id, start_at, end_at, state)
            VALUES (?1, ?2, ?3, ?4)
            ON CONFLICT(task_id) DO UPDATE SET
                start_at = excluded.start_at,
                end_at   = excluded.end_at,
                state    = excluded.state
            "#,
            task_id,
            self.start_at,
            self.end_at,
            self.state,
        )
        .execute(db)
        .await?;

        Ok(())
    }
}
