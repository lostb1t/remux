use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use std::time::Duration;
use uuid::Uuid;

use crate::addons::tracking::{TrackingError, TrackingEventKind};

/// Attempts before a row is parked as `failed_retryable` and stops being
/// retried. Roughly a day of backoff, so a provider outage is ridden out but a
/// permanently broken one does not retry forever.
pub const MAX_ATTEMPTS: i64 = 12;

const BASE_BACKOFF_SECS: i64 = 30;
const MAX_BACKOFF_SECS: i64 = 6 * 60 * 60;

#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Default,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum OutboxStatus {
    #[default]
    Pending,
    Delivered,
    /// Ran out of attempts. Kept, not deleted: this is the dead-letter view.
    FailedRetryable,
    /// The provider said retrying cannot help.
    FailedPermanent,
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct TrackingOutbox {
    pub id: Uuid,
    pub user_media_tracker_id: Uuid,
    pub event_kind: TrackingEventKind,
    pub payload: String,
    pub status: OutboxStatus,
    pub attempts: i64,
    pub next_attempt_at: NaiveDateTime,
    pub last_error: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub delivered_at: Option<NaiveDateTime>,
}

const COLS: &str = "id, user_media_tracker_id, event_kind, payload, status, attempts, \
     next_attempt_at, last_error, created_at, updated_at, delivered_at";

/// Delay before attempt `attempts + 1`, doubling from 30s and capped at 6h.
/// `retry_after` from the provider wins when it asks for longer; honouring a
/// shorter hint would defeat our own backoff.
pub fn backoff(attempts: i64, retry_after: Option<Duration>) -> Duration {
    let shift = attempts.clamp(0, 20) as u32;
    let secs = BASE_BACKOFF_SECS
        .saturating_mul(1i64 << shift.min(20))
        .min(MAX_BACKOFF_SECS);
    let ours = Duration::from_secs(secs as u64);
    match retry_after {
        Some(theirs) if theirs > ours => theirs,
        _ => ours,
    }
}

impl TrackingOutbox {
    pub fn new(
        user_media_tracker_id: Uuid,
        event_kind: TrackingEventKind,
        payload: String,
    ) -> Self {
        let now = Utc::now().naive_utc();
        Self {
            id: crate::common::get_uuid(),
            user_media_tracker_id,
            event_kind,
            payload,
            status: OutboxStatus::Pending,
            attempts: 0,
            // Due immediately; the worker is also poked directly on enqueue.
            next_attempt_at: now,
            last_error: None,
            created_at: now,
            updated_at: now,
            delivered_at: None,
        }
    }

    pub async fn insert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO tracking_outbox \
             (id, user_media_tracker_id, event_kind, payload, status, attempts, \
              next_attempt_at, last_error, created_at, updated_at, delivered_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(self.id)
        .bind(self.user_media_tracker_id)
        .bind(self.event_kind)
        .bind(&self.payload)
        .bind(self.status)
        .bind(self.attempts)
        .bind(self.next_attempt_at)
        .bind(&self.last_error)
        .bind(self.created_at)
        .bind(self.updated_at)
        .bind(self.delivered_at)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM tracking_outbox WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(db)
        .await?)
    }

    pub async fn due(db: &SqlitePool, limit: i64) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM tracking_outbox \
             WHERE status = 'pending' AND next_attempt_at <= ?1 \
             ORDER BY next_attempt_at ASC LIMIT ?2"
        ))
        .bind(Utc::now().naive_utc())
        .bind(limit)
        .fetch_all(db)
        .await?)
    }

    pub async fn list_for_connection(
        db: &SqlitePool,
        user_media_tracker_id: Uuid,
        limit: i64,
    ) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM tracking_outbox WHERE user_media_tracker_id = ?1 \
             ORDER BY created_at DESC LIMIT ?2"
        ))
        .bind(user_media_tracker_id)
        .bind(limit)
        .fetch_all(db)
        .await?)
    }

    pub async fn mark_delivered(db: &SqlitePool, id: Uuid) -> Result<()> {
        let now = Utc::now().naive_utc();
        sqlx::query(
            "UPDATE tracking_outbox \
             SET status = 'delivered', delivered_at = ?2, last_error = NULL, \
                 updated_at = ?2 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(now)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Record a failed attempt and decide whether this row gets another go.
    ///
    /// Returns the status it landed in. Retryable rows stay `pending` with a
    /// later `next_attempt_at` until they run out of attempts; permanent ones
    /// stop immediately, since retrying cannot fix them.
    pub async fn record_failure(
        db: &SqlitePool,
        id: Uuid,
        attempts: i64,
        err: &TrackingError,
    ) -> Result<OutboxStatus> {
        let now = Utc::now().naive_utc();
        let attempts = attempts + 1;

        let (status, next_attempt_at) = match err {
            TrackingError::Permanent { .. } => (OutboxStatus::FailedPermanent, now),
            TrackingError::Retryable { retry_after, .. } => {
                if attempts >= MAX_ATTEMPTS {
                    (OutboxStatus::FailedRetryable, now)
                } else {
                    let wait = backoff(attempts, *retry_after);
                    let next = now
                        + chrono::Duration::from_std(wait).unwrap_or_else(|_| {
                            chrono::Duration::seconds(MAX_BACKOFF_SECS)
                        });
                    (OutboxStatus::Pending, next)
                }
            }
        };

        sqlx::query(
            "UPDATE tracking_outbox \
             SET status = ?2, attempts = ?3, next_attempt_at = ?4, \
                 last_error = ?5, updated_at = ?6 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(status)
        .bind(attempts)
        .bind(next_attempt_at)
        .bind(err.to_string())
        .bind(now)
        .execute(db)
        .await?;
        Ok(status)
    }

    /// Trim delivered rows older than `keep_days`. Failed rows are left alone:
    /// they are what the activity views show.
    pub async fn purge_delivered(db: &SqlitePool, keep_days: i64) -> Result<u64> {
        let cutoff = Utc::now().naive_utc() - chrono::Duration::days(keep_days);
        let res = sqlx::query(
            "DELETE FROM tracking_outbox \
             WHERE status = 'delivered' AND delivered_at < ?1",
        )
        .bind(cutoff)
        .execute(db)
        .await?;
        Ok(res.rows_affected())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_doubles_and_caps() {
        assert_eq!(backoff(0, None), Duration::from_secs(30));
        assert_eq!(backoff(1, None), Duration::from_secs(60));
        assert_eq!(backoff(4, None), Duration::from_secs(480));
        // Capped rather than growing without bound.
        assert_eq!(
            backoff(30, None),
            Duration::from_secs(MAX_BACKOFF_SECS as u64)
        );
    }

    #[test]
    fn a_longer_provider_hint_wins_but_a_shorter_one_does_not() {
        // Honouring a shorter Retry-After than our own backoff would defeat the
        // point of backing off.
        assert_eq!(
            backoff(0, Some(Duration::from_secs(300))),
            Duration::from_secs(300)
        );
        assert_eq!(
            backoff(4, Some(Duration::from_secs(5))),
            Duration::from_secs(480)
        );
    }

    // --- state machine, against a real database ---

    use crate::integration_test::new_test_server;
    use sqlx::SqlitePool;

    async fn seed(db: &SqlitePool) -> Uuid {
        let addon = crate::common::get_uuid();
        sqlx::query(
            "INSERT INTO addons (id, name, preset, resources, types, enabled, \
             priority, created_at, updated_at, system, is_default) \
             VALUES (?1, 'yamtrack', '{\"kind\":\"yamtrack\",\"config\":{}}', \
             '[]', '[]', 1, 0, datetime('now'), datetime('now'), 0, 1)",
        )
        .bind(addon)
        .execute(db)
        .await
        .unwrap();

        let mut user = crate::db::User::new_with_password(
            String::new(),
            "alice".into(),
            "pw",
            None,
        )
        .unwrap();
        user.save(db)
            .await
            .unwrap();

        let conn = crate::db::UserMediaTracker::new(
            user.id,
            addon,
            Default::default(),
            vec![TrackingEventKind::PlaybackStop],
        );
        conn.upsert(db)
            .await
            .unwrap();
        conn.id
    }

    async fn queue(db: &SqlitePool, conn: Uuid) -> TrackingOutbox {
        let row =
            TrackingOutbox::new(conn, TrackingEventKind::PlaybackStop, "{}".into());
        row.insert(db)
            .await
            .unwrap();
        row
    }

    #[tokio::test]
    async fn a_retryable_failure_stays_pending_and_is_deferred() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        assert_eq!(
            TrackingOutbox::due(db, 10)
                .await
                .unwrap()
                .len(),
            1,
            "a fresh row is due immediately"
        );

        let status = TrackingOutbox::record_failure(
            db,
            row.id,
            0,
            &TrackingError::retryable("503"),
        )
        .await
        .unwrap();
        assert_eq!(status, OutboxStatus::Pending);

        let got = TrackingOutbox::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.attempts, 1);
        assert!(
            got.next_attempt_at > got.created_at,
            "should have been pushed into the future"
        );
        assert!(
            TrackingOutbox::due(db, 10)
                .await
                .unwrap()
                .is_empty(),
            "must not be retried until its backoff elapses"
        );
    }

    #[tokio::test]
    async fn a_permanent_failure_stops_immediately() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        let status = TrackingOutbox::record_failure(
            db,
            row.id,
            0,
            &TrackingError::reauth("401"),
        )
        .await
        .unwrap();
        assert_eq!(status, OutboxStatus::FailedPermanent);
        assert!(
            TrackingOutbox::due(db, 10)
                .await
                .unwrap()
                .is_empty(),
            "retrying cannot fix bad credentials"
        );
    }

    #[tokio::test]
    async fn retryable_failures_dead_letter_once_attempts_run_out() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        let mut status = OutboxStatus::Pending;
        for attempt in 0..MAX_ATTEMPTS {
            status = TrackingOutbox::record_failure(
                db,
                row.id,
                attempt,
                &TrackingError::retryable("503"),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            status,
            OutboxStatus::FailedRetryable,
            "should stop rather than retry forever"
        );

        // Kept, not deleted: this is what the activity view shows.
        let got = TrackingOutbox::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.attempts, MAX_ATTEMPTS);
        assert_eq!(
            got.last_error
                .as_deref(),
            Some("503")
        );
    }

    #[tokio::test]
    async fn delivery_clears_the_error_and_leaves_the_row_for_audit() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        TrackingOutbox::record_failure(db, row.id, 0, &TrackingError::retryable("503"))
            .await
            .unwrap();
        TrackingOutbox::mark_delivered(db, row.id)
            .await
            .unwrap();

        let got = TrackingOutbox::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, OutboxStatus::Delivered);
        assert!(
            got.last_error
                .is_none()
        );
        assert!(
            got.delivered_at
                .is_some()
        );
        assert!(
            TrackingOutbox::due(db, 10)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn purge_trims_delivered_rows_but_keeps_failures() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let delivered = queue(db, conn).await;
        let failed = queue(db, conn).await;

        TrackingOutbox::mark_delivered(db, delivered.id)
            .await
            .unwrap();
        TrackingOutbox::record_failure(
            db,
            failed.id,
            0,
            &TrackingError::permanent("nope"),
        )
        .await
        .unwrap();
        // Backdate the delivered row past the retention window.
        sqlx::query("UPDATE tracking_outbox SET delivered_at = ?2 WHERE id = ?1")
            .bind(delivered.id)
            .bind(Utc::now().naive_utc() - chrono::Duration::days(30))
            .execute(db)
            .await
            .unwrap();

        let purged = TrackingOutbox::purge_delivered(db, 7)
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert!(
            TrackingOutbox::get(db, failed.id)
                .await
                .unwrap()
                .is_some(),
            "failures are the audit trail and must survive"
        );
    }

    #[tokio::test]
    async fn deleting_a_connection_takes_its_queue_with_it() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        crate::db::UserMediaTracker::delete(db, conn)
            .await
            .unwrap();

        assert!(
            TrackingOutbox::get(db, row.id)
                .await
                .unwrap()
                .is_none(),
            "disconnecting must not leave deliveries queued for a dead connection"
        );
    }

    #[tokio::test]
    async fn the_sync_task_is_scheduled() {
        // Registered but untriggered, the worker would only ever run when an
        // admin pressed the button, so nothing would actually be retried.
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM task_triggers WHERE task_id = 'TrackingSync'",
        )
        .fetch_one(
            &guard
                .0
                .db,
        )
        .await
        .unwrap();
        assert_eq!(count, 1, "TrackingSync has no schedule");
    }
}
