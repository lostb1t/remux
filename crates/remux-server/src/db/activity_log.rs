//! Server activity/audit log.
//!
//! Backs Jellyfin's `GET /System/ActivityLog/Entries` with real events recorded
//! at genuine sites across the server (logins, playback start/stop, scheduled-task
//! failures, user create/delete). Recording is deliberately non-fatal: a failed
//! insert is logged and swallowed so instrumentation can never break the request
//! that triggered it.

use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::Serialize;
use sqlx::SqlitePool;

/// Jellyfin `LogLevel` — the severity of an activity-log entry. `Display`
/// round-trips the exact PascalCase strings Jellyfin uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
pub enum ActivitySeverity {
    Information,
    Warning,
    Error,
}

/// The activity event types Remux records. The variant name is the wire value
/// (Jellyfin `Type`) verbatim, e.g. `AuthenticationSucceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, strum_macros::Display)]
pub enum ActivityKind {
    AuthenticationSucceeded,
    VideoPlayback,
    VideoPlaybackStopped,
    ScheduledTaskFailed,
    UserCreated,
    UserDeleted,
}

/// A new event to record. `date` is stamped at insert time.
#[derive(Debug, Clone)]
pub struct NewActivity {
    pub name: String,
    pub kind: ActivityKind,
    pub severity: ActivitySeverity,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    pub item_id: Option<String>,
    pub user_id: Option<String>,
}

impl NewActivity {
    /// Convenience constructor for the common case: a name + kind at
    /// `Information` severity, no extra detail.
    pub fn info(name: impl Into<String>, kind: ActivityKind) -> Self {
        Self {
            name: name.into(),
            kind,
            severity: ActivitySeverity::Information,
            overview: None,
            short_overview: None,
            item_id: None,
            user_id: None,
        }
    }

    /// Attach the acting user's id (as a string GUID).
    pub fn with_user(mut self, user_id: impl Into<String>) -> Self {
        self.user_id = Some(user_id.into());
        self
    }

    /// Attach the related item's id.
    pub fn with_item(mut self, item_id: impl Into<String>) -> Self {
        self.item_id = Some(item_id.into());
        self
    }
}

/// One row of the activity log, serialized as a Jellyfin `ActivityLogEntry`.
#[derive(Debug, Clone, Serialize, sqlx::FromRow)]
#[serde(rename_all = "PascalCase")]
pub struct ActivityLogEntry {
    pub id: i64,
    pub name: String,
    pub overview: Option<String>,
    pub short_overview: Option<String>,
    #[sqlx(rename = "type")]
    #[serde(rename = "Type")]
    pub type_: String,
    pub item_id: Option<String>,
    pub user_id: Option<String>,
    pub date: DateTime<Utc>,
    pub severity: String,
}

pub struct ActivityLog;

impl ActivityLog {
    /// Insert an event. Prefer [`ActivityLog::record_ignore`] at call sites so a
    /// logging failure never surfaces to the user.
    pub async fn record(db: &SqlitePool, new: NewActivity) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO activity_log
                (name, overview, short_overview, type, item_id, user_id, date, severity)
            VALUES
                (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
            "#,
        )
        .bind(&new.name)
        .bind(&new.overview)
        .bind(&new.short_overview)
        .bind(
            new.kind
                .to_string(),
        )
        .bind(&new.item_id)
        .bind(&new.user_id)
        .bind(Utc::now())
        .bind(
            new.severity
                .to_string(),
        )
        .execute(db)
        .await?;
        Ok(())
    }

    /// Fire-and-forget recorder: records an event, logging (never propagating)
    /// any failure. Safe to call inline from a request handler.
    pub async fn record_ignore(db: &SqlitePool, new: NewActivity) {
        if let Err(e) = Self::record(db, new).await {
            tracing::warn!("failed to record activity-log entry: {e:#}");
        }
    }

    /// Page through entries, newest first, with the optional Jellyfin filters
    /// `minDate` and `hasUserId`. Returns `(page, total_matching)`.
    pub async fn query(
        db: &SqlitePool,
        start_index: i64,
        limit: i64,
        min_date: Option<DateTime<Utc>>,
        has_user_id: Option<bool>,
    ) -> Result<(Vec<ActivityLogEntry>, i64)> {
        let mut filters: Vec<&str> = Vec::new();
        if min_date.is_some() {
            filters.push("date >= ?");
        }
        match has_user_id {
            Some(true) => filters.push("user_id IS NOT NULL"),
            Some(false) => filters.push("user_id IS NULL"),
            None => {}
        }
        let where_sql = if filters.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", filters.join(" AND "))
        };

        let count_sql = format!("SELECT COUNT(*) FROM activity_log {where_sql}");
        let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
        if let Some(d) = min_date {
            count_q = count_q.bind(d);
        }
        let total = count_q
            .fetch_one(db)
            .await?;

        let list_sql = format!(
            "SELECT id, name, overview, short_overview, type, item_id, user_id, date, severity \
             FROM activity_log {where_sql} ORDER BY date DESC, id DESC LIMIT ? OFFSET ?"
        );
        let mut list_q = sqlx::query_as::<_, ActivityLogEntry>(&list_sql);
        if let Some(d) = min_date {
            list_q = list_q.bind(d);
        }
        let items = list_q
            .bind(limit)
            .bind(start_index)
            .fetch_all(db)
            .await?;

        Ok((items, total))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn mem_db() -> SqlitePool {
        let pool = SqlitePool::connect("sqlite::memory:")
            .await
            .unwrap();
        sqlx::query(
            r#"CREATE TABLE activity_log (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                name TEXT NOT NULL, overview TEXT, short_overview TEXT,
                type TEXT NOT NULL, item_id TEXT, user_id TEXT,
                date TEXT NOT NULL, severity TEXT NOT NULL DEFAULT 'Information'
            )"#,
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn records_and_queries_newest_first() {
        let db = mem_db().await;

        ActivityLog::record(&db, NewActivity::info("first", ActivityKind::UserCreated))
            .await
            .unwrap();
        ActivityLog::record(
            &db,
            NewActivity::info("second", ActivityKind::AuthenticationSucceeded)
                .with_user("user-1"),
        )
        .await
        .unwrap();

        let (items, total) = ActivityLog::query(&db, 0, 100, None, None)
            .await
            .unwrap();
        assert_eq!(total, 2);
        // Newest first (id DESC tiebreak on identical timestamps).
        assert_eq!(items[0].name, "second");
        assert_eq!(items[0].type_, "AuthenticationSucceeded");
        assert_eq!(items[1].name, "first");
    }

    #[tokio::test]
    async fn has_user_id_filter_and_paging() {
        let db = mem_db().await;
        for i in 0..3 {
            ActivityLog::record(
                &db,
                NewActivity::info(
                    format!("sys-{i}"),
                    ActivityKind::ScheduledTaskFailed,
                ),
            )
            .await
            .unwrap();
        }
        ActivityLog::record(
            &db,
            NewActivity::info("login", ActivityKind::AuthenticationSucceeded)
                .with_user("u"),
        )
        .await
        .unwrap();

        // Only user events.
        let (user_items, user_total) =
            ActivityLog::query(&db, 0, 100, None, Some(true))
                .await
                .unwrap();
        assert_eq!(user_total, 1);
        assert_eq!(user_items.len(), 1);
        assert_eq!(user_items[0].name, "login");

        // Only system events.
        let (_sys_items, sys_total) =
            ActivityLog::query(&db, 0, 100, None, Some(false))
                .await
                .unwrap();
        assert_eq!(sys_total, 3);

        // Paging: limit 2, offset 0 then 2 across all 4 rows.
        let (page1, total) = ActivityLog::query(&db, 0, 2, None, None)
            .await
            .unwrap();
        assert_eq!(total, 4);
        assert_eq!(page1.len(), 2);
        let (page2, _) = ActivityLog::query(&db, 2, 2, None, None)
            .await
            .unwrap();
        assert_eq!(page2.len(), 2);
    }
}
