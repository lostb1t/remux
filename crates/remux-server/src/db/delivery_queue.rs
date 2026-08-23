use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, Row, SqlitePool, sqlite::SqliteRow};
use std::time::Duration;
use uuid::Uuid;

use crate::addons::media_tracker::{MediaTrackerError, MediaTrackerEvent};

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
pub enum DeliveryStatus {
    #[default]
    Pending,
    Delivered,
    /// Ran out of attempts. Kept, not deleted: this is the dead-letter view.
    FailedRetryable,
    /// The provider said retrying cannot help.
    FailedPermanent,
}

/// Which deliverer a queued row belongs to. Stored in its own column so the
/// worker can tell what a row is without parsing its payload first.
#[derive(
    strum_macros::EnumString,
    strum_macros::Display,
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    sqlx::Type,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[sqlx(type_name = "TEXT", rename_all = "snake_case")]
pub enum DeliveryKind {
    MediaTracker,
}

/// What one media tracker delivery carries. The item is a reference rather than a
/// snapshot, so the target is built at delivery, where its external ids can
/// still be completed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MediaTrackerPayload {
    pub media_id: Uuid,
    pub event: MediaTrackerEvent,
}

/// A queued delivery and everything its deliverer needs to make it.
///
/// The retry machinery wrapped around this is kind-agnostic; only the sync
/// task's dispatch cares which variant it holds.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum QueueKind {
    /// Watch activity headed for one of a user's connected media trackers.
    MediaTracker {
        user_media_tracker_id: Uuid,
        payload: MediaTrackerPayload,
    },
}

impl QueueKind {
    pub fn kind(&self) -> DeliveryKind {
        match self {
            Self::MediaTracker { .. } => DeliveryKind::MediaTracker,
        }
    }

    /// The owner column for this variant, which is what makes a disconnect
    /// take the backlog with it.
    fn user_media_tracker_id(&self) -> Option<Uuid> {
        match self {
            Self::MediaTracker {
                user_media_tracker_id,
                ..
            } => Some(*user_media_tracker_id),
        }
    }

    /// The variant's body, minus whatever already has its own column.
    fn body(&self) -> Result<String> {
        Ok(match self {
            Self::MediaTracker { payload, .. } => serde_json::to_string(payload)?,
        })
    }

    /// Rebuild a variant from the three columns that hold it. A row that will
    /// not read back is a decode error.
    fn parse(
        kind: DeliveryKind,
        user_media_tracker_id: Option<Uuid>,
        body: &str,
    ) -> Result<Self, sqlx::Error> {
        let decode =
            |col: &'static str, e: Box<dyn std::error::Error + Send + Sync>| {
                sqlx::Error::ColumnDecode {
                    index: col.into(),
                    source: e,
                }
            };
        match kind {
            DeliveryKind::MediaTracker => Ok(Self::MediaTracker {
                user_media_tracker_id: user_media_tracker_id.ok_or_else(|| {
                    decode(
                        "user_media_tracker_id",
                        "a tracker row without an owner".into(),
                    )
                })?,
                payload: serde_json::from_str(body)
                    .map_err(|e| decode("payload", Box::new(e)))?,
            }),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeliveryQueue {
    pub id: Uuid,
    pub kind: QueueKind,
    pub status: DeliveryStatus,
    pub attempts: i64,
    pub next_attempt_at: NaiveDateTime,
    pub last_error: Option<String>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
    pub delivered_at: Option<NaiveDateTime>,
}

impl FromRow<'_, SqliteRow> for DeliveryQueue {
    fn from_row(row: &SqliteRow) -> Result<Self, sqlx::Error> {
        Ok(Self {
            id: row.try_get("id")?,
            kind: QueueKind::parse(
                row.try_get("kind")?,
                row.try_get("user_media_tracker_id")?,
                row.try_get::<String, _>("payload")?
                    .as_str(),
            )?,
            status: row.try_get("status")?,
            attempts: row.try_get("attempts")?,
            next_attempt_at: row.try_get("next_attempt_at")?,
            last_error: row.try_get("last_error")?,
            created_at: row.try_get("created_at")?,
            updated_at: row.try_get("updated_at")?,
            delivered_at: row.try_get("delivered_at")?,
        })
    }
}

const COLS: &str = "id, kind, user_media_tracker_id, payload, status, attempts, \
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

impl DeliveryQueue {
    pub fn new(kind: QueueKind) -> Self {
        let now = Utc::now().naive_utc();
        Self {
            id: crate::common::get_uuid(),
            kind,
            status: DeliveryStatus::Pending,
            attempts: 0,
            // Due immediately, for the next sweep to pick up.
            next_attempt_at: now,
            last_error: None,
            created_at: now,
            updated_at: now,
            delivered_at: None,
        }
    }

    pub async fn insert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO delivery_queue \
             (id, kind, user_media_tracker_id, payload, status, attempts, \
              next_attempt_at, last_error, created_at, updated_at, delivered_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        )
        .bind(self.id)
        .bind(
            self.kind
                .kind(),
        )
        .bind(
            self.kind
                .user_media_tracker_id(),
        )
        .bind(
            self.kind
                .body()?,
        )
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
            "SELECT {COLS} FROM delivery_queue WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(db)
        .await?)
    }

    /// Due rows, oldest first. `created_at` breaks ties so two rows queued in
    /// the same tick come back in the order they were written: the drain pass
    /// delivers one tracker's rows in exactly this order.
    pub async fn due(db: &SqlitePool, limit: i64) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM delivery_queue \
             WHERE status = 'pending' AND next_attempt_at <= ?1 \
             ORDER BY next_attempt_at ASC, created_at ASC LIMIT ?2"
        ))
        .bind(Utc::now().naive_utc())
        .bind(limit)
        .fetch_all(db)
        .await?)
    }

    pub async fn list_for_media_tracker(
        db: &SqlitePool,
        user_media_tracker_id: Uuid,
        limit: i64,
    ) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM delivery_queue WHERE user_media_tracker_id = ?1 \
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
            "UPDATE delivery_queue \
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
        err: &MediaTrackerError,
    ) -> Result<DeliveryStatus> {
        let now = Utc::now().naive_utc();
        // `attempts` is the count *before* this failure, which is also the
        // number of waits already served: the first failure backs off by the
        // base delay, not double it.
        let attempted = attempts + 1;

        let (status, next_attempt_at) = match err {
            MediaTrackerError::Permanent { .. } => {
                (DeliveryStatus::FailedPermanent, now)
            }
            MediaTrackerError::Retryable { retry_after, .. } => {
                if attempted >= MAX_ATTEMPTS {
                    (DeliveryStatus::FailedRetryable, now)
                } else {
                    let wait = backoff(attempts, *retry_after);
                    let next = now
                        + chrono::Duration::from_std(wait).unwrap_or_else(|_| {
                            chrono::Duration::seconds(MAX_BACKOFF_SECS)
                        });
                    (DeliveryStatus::Pending, next)
                }
            }
        };

        sqlx::query(
            "UPDATE delivery_queue \
             SET status = ?2, attempts = ?3, next_attempt_at = ?4, \
                 last_error = ?5, updated_at = ?6 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(status)
        .bind(attempted)
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
            "DELETE FROM delivery_queue \
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
        assert_eq!(
            backoff(30, None),
            Duration::from_secs(MAX_BACKOFF_SECS as u64)
        );
    }

    #[test]
    fn a_longer_provider_hint_wins_but_a_shorter_one_does_not() {
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

    use crate::{
        addons::media_tracker::MediaTrackerEventKind, integration_test::new_test_server,
    };
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
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        conn.upsert(db)
            .await
            .unwrap();
        conn.id
    }

    pub(crate) fn tracker_payload() -> MediaTrackerPayload {
        MediaTrackerPayload {
            media_id: crate::common::get_uuid(),
            event: MediaTrackerEvent::PlaybackStop {
                position_ticks: 42,
                played: true,
            },
        }
    }

    async fn queue(db: &SqlitePool, conn: Uuid) -> DeliveryQueue {
        let row = DeliveryQueue::new(QueueKind::MediaTracker {
            user_media_tracker_id: conn,
            payload: tracker_payload(),
        });
        row.insert(db)
            .await
            .unwrap();
        row
    }

    #[tokio::test]
    async fn a_row_round_trips_through_its_columns() {
        // Insert splits the payload across three columns; only a read-back
        // shows that is lossless.
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let conn = seed(db).await;
        let row = queue(db, conn).await;

        let got = DeliveryQueue::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.kind, row.kind);
        assert_eq!(
            got.kind
                .kind(),
            DeliveryKind::MediaTracker
        );
    }

    #[tokio::test]
    async fn a_media_tracker_row_cannot_be_stored_without_its_owner() {
        // Nothing would cascade it away, and delivery would have nowhere to
        // send it. The kind is bound rather than written out, so renaming the
        // variant without migrating the CHECK fails here instead of quietly
        // leaving it matching nothing.
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        seed(db).await;

        let err = sqlx::query(
            "INSERT INTO delivery_queue \
             (id, kind, user_media_tracker_id, payload, status, attempts, \
              next_attempt_at, created_at, updated_at) \
             VALUES (?1, ?2, NULL, '{}', 'pending', 0, datetime('now'), \
                     datetime('now'), datetime('now'))",
        )
        .bind(crate::common::get_uuid())
        .bind(DeliveryKind::MediaTracker.to_string())
        .execute(db)
        .await
        .unwrap_err();
        assert!(
            err.to_string()
                .to_lowercase()
                .contains("constraint"),
            "expected the CHECK to reject it, got: {err}"
        );
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
            DeliveryQueue::due(db, 10)
                .await
                .unwrap()
                .len(),
            1,
            "a fresh row is due immediately"
        );

        let status = DeliveryQueue::record_failure(
            db,
            row.id,
            0,
            &MediaTrackerError::retryable("503"),
        )
        .await
        .unwrap();
        assert_eq!(status, DeliveryStatus::Pending);

        let got = DeliveryQueue::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.attempts, 1);
        assert_eq!(
            (got.next_attempt_at - got.updated_at).num_seconds(),
            BASE_BACKOFF_SECS,
            "the first retry waits the base delay, not a doubled one"
        );
        assert!(
            DeliveryQueue::due(db, 10)
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

        let status = DeliveryQueue::record_failure(
            db,
            row.id,
            0,
            &MediaTrackerError::reauth("401"),
        )
        .await
        .unwrap();
        assert_eq!(status, DeliveryStatus::FailedPermanent);
        assert!(
            DeliveryQueue::due(db, 10)
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

        let mut status = DeliveryStatus::Pending;
        for attempt in 0..MAX_ATTEMPTS {
            status = DeliveryQueue::record_failure(
                db,
                row.id,
                attempt,
                &MediaTrackerError::retryable("503"),
            )
            .await
            .unwrap();
        }
        assert_eq!(
            status,
            DeliveryStatus::FailedRetryable,
            "should stop rather than retry forever"
        );

        // Kept, not deleted: this is what the activity view shows.
        let got = DeliveryQueue::get(db, row.id)
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

        DeliveryQueue::record_failure(
            db,
            row.id,
            0,
            &MediaTrackerError::retryable("503"),
        )
        .await
        .unwrap();
        DeliveryQueue::mark_delivered(db, row.id)
            .await
            .unwrap();

        let got = DeliveryQueue::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, DeliveryStatus::Delivered);
        assert!(
            got.last_error
                .is_none()
        );
        assert!(
            got.delivered_at
                .is_some()
        );
        assert!(
            DeliveryQueue::due(db, 10)
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

        DeliveryQueue::mark_delivered(db, delivered.id)
            .await
            .unwrap();
        DeliveryQueue::record_failure(
            db,
            failed.id,
            0,
            &MediaTrackerError::permanent("nope"),
        )
        .await
        .unwrap();
        // Backdate the delivered row past the retention window.
        sqlx::query("UPDATE delivery_queue SET delivered_at = ?2 WHERE id = ?1")
            .bind(delivered.id)
            .bind(Utc::now().naive_utc() - chrono::Duration::days(30))
            .execute(db)
            .await
            .unwrap();

        let purged = DeliveryQueue::purge_delivered(db, 7)
            .await
            .unwrap();
        assert_eq!(purged, 1);
        assert!(
            DeliveryQueue::get(db, failed.id)
                .await
                .unwrap()
                .is_some(),
            "failures are the audit trail and must survive"
        );
    }

    #[tokio::test]
    async fn deleting_a_media_tracker_takes_its_queue_with_it() {
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
            DeliveryQueue::get(db, row.id)
                .await
                .unwrap()
                .is_none(),
            "disconnecting must not leave deliveries queued for a dead media tracker"
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
            "SELECT COUNT(*) FROM task_triggers WHERE task_id = 'DeliveryQueueSync'",
        )
        .fetch_one(
            &guard
                .0
                .db,
        )
        .await
        .unwrap();
        assert_eq!(count, 1, "DeliveryQueueSync has no schedule");
    }
}
