use anyhow::Result;
use chrono::{NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::addons::media_tracker::{
    MediaTrackerCredentials, MediaTrackerError, MediaTrackerEventKind,
};

/// Health of one user's connection to a media tracker, as shown on their
/// connected services page.
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
pub enum MediaTrackerStatus {
    #[default]
    Disconnected,
    Connected,
    /// Last sync failed for a reason the user cannot fix.
    Error,
    /// Credentials rejected. The UI offers a reconnect.
    AuthExpired,
}

/// Which half of `MediaTrackerError` the last failure was, so the UI can tell a
/// blip from something needing attention.
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
pub enum MediaTrackerErrorKind {
    Retryable,
    Permanent,
}

impl From<&MediaTrackerError> for MediaTrackerErrorKind {
    fn from(err: &MediaTrackerError) -> Self {
        if err.is_retryable() {
            Self::Retryable
        } else {
            Self::Permanent
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct UserMediaTracker {
    pub id: Uuid,
    pub addon_id: Uuid,
    pub user_id: Uuid,
    pub status: MediaTrackerStatus,
    #[sqlx(json)]
    #[serde(skip_serializing)]
    pub credentials: MediaTrackerCredentials,
    #[sqlx(json)]
    pub event_filters: Vec<MediaTrackerEventKind>,
    pub last_success_at: Option<NaiveDateTime>,
    pub last_error_at: Option<NaiveDateTime>,
    pub last_error: Option<String>,
    pub last_error_kind: Option<MediaTrackerErrorKind>,
    pub created_at: NaiveDateTime,
    pub updated_at: NaiveDateTime,
}

const COLS: &str = "id, addon_id, user_id, status, credentials, event_filters, \
     last_success_at, last_error_at, last_error, last_error_kind, created_at, updated_at";

impl UserMediaTracker {
    pub fn new(
        user_id: Uuid,
        addon_id: Uuid,
        credentials: MediaTrackerCredentials,
        event_filters: Vec<MediaTrackerEventKind>,
    ) -> Self {
        let now = Utc::now().naive_utc();
        Self {
            id: crate::common::get_uuid(),
            addon_id,
            user_id,
            status: MediaTrackerStatus::Connected,
            credentials,
            event_filters,
            last_success_at: None,
            last_error_at: None,
            last_error: None,
            last_error_kind: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Whether this connection should receive `kind`.
    pub fn wants(&self, kind: MediaTrackerEventKind) -> bool {
        self.status == MediaTrackerStatus::Connected
            && self
                .event_filters
                .contains(&kind)
    }

    pub async fn get(db: &SqlitePool, id: Uuid) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM user_media_trackers WHERE id = ?1"
        ))
        .bind(id)
        .fetch_optional(db)
        .await?)
    }

    pub async fn get_for_user_and_addon(
        db: &SqlitePool,
        user_id: Uuid,
        addon_id: Uuid,
    ) -> Result<Option<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM user_media_trackers WHERE user_id = ?1 AND addon_id = ?2"
        ))
        .bind(user_id)
        .bind(addon_id)
        .fetch_optional(db)
        .await?)
    }

    pub async fn list_for_user(db: &SqlitePool, user_id: Uuid) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM user_media_trackers WHERE user_id = ?1 \
             ORDER BY created_at ASC"
        ))
        .bind(user_id)
        .fetch_all(db)
        .await?)
    }

    pub async fn list_for_addon(db: &SqlitePool, addon_id: Uuid) -> Result<Vec<Self>> {
        Ok(sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM user_media_trackers WHERE addon_id = ?1 \
             ORDER BY created_at ASC"
        ))
        .bind(addon_id)
        .fetch_all(db)
        .await?)
    }

    /// Every connection that wants `kind`, across all users. The dispatcher's
    /// fan-out query.
    pub async fn list_subscribed(
        db: &SqlitePool,
        addon_id: Uuid,
        kind: MediaTrackerEventKind,
    ) -> Result<Vec<Self>> {
        let rows = Self::list_for_addon(db, addon_id).await?;
        Ok(rows
            .into_iter()
            .filter(|r| r.wants(kind))
            .collect())
    }

    /// Insert, or replace the credentials and filters of an existing
    /// connection. Reconnecting keeps the row so its id stays stable for
    /// anything referencing it.
    pub async fn upsert(&self, db: &SqlitePool) -> Result<()> {
        sqlx::query(
            "INSERT INTO user_media_trackers \
             (id, addon_id, user_id, status, credentials, event_filters, \
              last_success_at, last_error_at, last_error, last_error_kind, \
              created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12) \
             ON CONFLICT(addon_id, user_id) DO UPDATE SET \
                 status = excluded.status, \
                 credentials = excluded.credentials, \
                 event_filters = excluded.event_filters, \
                 updated_at = excluded.updated_at",
        )
        .bind(self.id)
        .bind(self.addon_id)
        .bind(self.user_id)
        .bind(self.status)
        .bind(sqlx::types::Json(&self.credentials))
        .bind(sqlx::types::Json(&self.event_filters))
        .bind(self.last_success_at)
        .bind(self.last_error_at)
        .bind(&self.last_error)
        .bind(self.last_error_kind)
        .bind(self.created_at)
        .bind(self.updated_at)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Replace the event filter without touching credentials, so changing what
    /// is synced never requires reconnecting.
    pub async fn set_event_filters(
        db: &SqlitePool,
        id: Uuid,
        filters: &[MediaTrackerEventKind],
    ) -> Result<()> {
        sqlx::query(
            "UPDATE user_media_trackers SET event_filters = ?2, updated_at = ?3 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(sqlx::types::Json(filters))
        .bind(Utc::now().naive_utc())
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn mark_success(db: &SqlitePool, id: Uuid) -> Result<()> {
        let now = Utc::now().naive_utc();
        sqlx::query(
            "UPDATE user_media_trackers \
             SET status = 'connected', last_success_at = ?2, \
                 last_error = NULL, last_error_at = NULL, last_error_kind = NULL, \
                 updated_at = ?2 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(now)
        .execute(db)
        .await?;
        Ok(())
    }

    /// A retryable failure leaves the status alone: the dispatcher will try
    /// again, and flipping to `error` would nag the user about a blip.
    pub async fn mark_failure(
        db: &SqlitePool,
        id: Uuid,
        err: &MediaTrackerError,
    ) -> Result<()> {
        let kind = MediaTrackerErrorKind::from(err);
        let status = match kind {
            MediaTrackerErrorKind::Retryable => None,
            MediaTrackerErrorKind::Permanent if err.requires_reauth() => {
                Some(MediaTrackerStatus::AuthExpired)
            }
            MediaTrackerErrorKind::Permanent => Some(MediaTrackerStatus::Error),
        };
        let now = Utc::now().naive_utc();
        sqlx::query(
            "UPDATE user_media_trackers \
             SET status = COALESCE(?2, status), last_error = ?3, \
                 last_error_at = ?4, last_error_kind = ?5, updated_at = ?4 \
             WHERE id = ?1",
        )
        .bind(id)
        .bind(status)
        .bind(err.to_string())
        .bind(now)
        .bind(kind)
        .execute(db)
        .await?;
        Ok(())
    }

    pub async fn delete(db: &SqlitePool, id: Uuid) -> Result<()> {
        sqlx::query("DELETE FROM user_media_trackers WHERE id = ?1")
            .bind(id)
            .execute(db)
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::integration_test::new_test_server;

    async fn seed_addon(db: &SqlitePool) -> Uuid {
        let id = crate::common::get_uuid();
        sqlx::query(
            "INSERT INTO addons (id, name, preset, resources, types, enabled, \
             priority, created_at, updated_at, system, is_default) \
             VALUES (?1, 'yamtrack', '{\"kind\":\"yamtrack\",\"config\":{}}', \
             '[]', '[]', 1, 0, datetime('now'), datetime('now'), 0, 1)",
        )
        .bind(id)
        .execute(db)
        .await
        .unwrap();
        id
    }

    async fn seed_user(db: &SqlitePool, name: &str) -> Uuid {
        let mut user =
            crate::db::User::new_with_password(String::new(), name.into(), "pw", None)
                .unwrap();
        user.save(db)
            .await
            .unwrap();
        user.id
    }

    fn creds(token: &str) -> MediaTrackerCredentials {
        MediaTrackerCredentials::new(serde_json::json!({ "token": token }))
    }

    #[tokio::test]
    async fn round_trips_through_the_database() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;

        let row = UserMediaTracker::new(
            user,
            addon,
            creds("abc"),
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        row.upsert(db)
            .await
            .unwrap();

        let got = UserMediaTracker::get_for_user_and_addon(db, user, addon)
            .await
            .unwrap()
            .expect("row should exist");
        assert_eq!(got.id, row.id);
        assert_eq!(got.status, MediaTrackerStatus::Connected);
        assert_eq!(
            got.credentials
                .get_str("token"),
            Some("abc")
        );
        assert_eq!(got.event_filters, vec![MediaTrackerEventKind::PlaybackStop]);
    }

    #[tokio::test]
    async fn reconnecting_replaces_credentials_and_keeps_the_row_id() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;

        let first = UserMediaTracker::new(user, addon, creds("old"), vec![]);
        first
            .upsert(db)
            .await
            .unwrap();

        let second = UserMediaTracker::new(
            user,
            addon,
            creds("new"),
            vec![MediaTrackerEventKind::MarkPlayed],
        );
        second
            .upsert(db)
            .await
            .unwrap();

        let rows = UserMediaTracker::list_for_user(db, user)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1, "the unique constraint should collapse these");
        assert_eq!(
            rows[0].id, first.id,
            "the original id must survive a reconnect"
        );
        assert_eq!(
            rows[0]
                .credentials
                .get_str("token"),
            Some("new")
        );
    }

    #[tokio::test]
    async fn users_do_not_see_each_others_connections() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let alice = seed_user(db, "alice").await;
        let bob = seed_user(db, "bob").await;

        UserMediaTracker::new(alice, addon, creds("a"), vec![])
            .upsert(db)
            .await
            .unwrap();
        UserMediaTracker::new(bob, addon, creds("b"), vec![])
            .upsert(db)
            .await
            .unwrap();

        let for_alice = UserMediaTracker::list_for_user(db, alice)
            .await
            .unwrap();
        assert_eq!(for_alice.len(), 1);
        assert_eq!(
            for_alice[0]
                .credentials
                .get_str("token"),
            Some("a")
        );
        assert_eq!(
            UserMediaTracker::list_for_addon(db, addon)
                .await
                .unwrap()
                .len(),
            2
        );
    }

    #[tokio::test]
    async fn only_connected_rows_matching_the_filter_are_dispatched_to() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let subscribed = seed_user(db, "subscribed").await;
        let unsubscribed = seed_user(db, "unsubscribed").await;
        let broken = seed_user(db, "broken").await;

        UserMediaTracker::new(
            subscribed,
            addon,
            creds("a"),
            vec![MediaTrackerEventKind::PlaybackStop],
        )
        .upsert(db)
        .await
        .unwrap();
        UserMediaTracker::new(
            unsubscribed,
            addon,
            creds("b"),
            vec![MediaTrackerEventKind::MarkPlayed],
        )
        .upsert(db)
        .await
        .unwrap();
        let broken_row = UserMediaTracker::new(
            broken,
            addon,
            creds("c"),
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        broken_row
            .upsert(db)
            .await
            .unwrap();
        UserMediaTracker::mark_failure(
            db,
            broken_row.id,
            &MediaTrackerError::reauth("401"),
        )
        .await
        .unwrap();

        let got = UserMediaTracker::list_subscribed(
            db,
            addon,
            MediaTrackerEventKind::PlaybackStop,
        )
        .await
        .unwrap();
        assert_eq!(
            got.len(),
            1,
            "auth_expired and unsubscribed must be skipped"
        );
        assert_eq!(got[0].user_id, subscribed);
    }

    #[tokio::test]
    async fn a_retryable_failure_does_not_disconnect_the_user() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;
        let row = UserMediaTracker::new(
            user,
            addon,
            creds("a"),
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        row.upsert(db)
            .await
            .unwrap();

        UserMediaTracker::mark_failure(
            db,
            row.id,
            &MediaTrackerError::retryable("503"),
        )
        .await
        .unwrap();

        let got = UserMediaTracker::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            got.status,
            MediaTrackerStatus::Connected,
            "a blip must not nag the user"
        );
        assert_eq!(got.last_error_kind, Some(MediaTrackerErrorKind::Retryable));
        assert!(
            got.wants(MediaTrackerEventKind::PlaybackStop),
            "still dispatched to while retrying"
        );
    }

    #[tokio::test]
    async fn a_permanent_failure_distinguishes_bad_credentials_from_everything_else() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let alice = seed_user(db, "alice").await;
        let bob = seed_user(db, "bob").await;

        let auth = UserMediaTracker::new(alice, addon, creds("a"), vec![]);
        auth.upsert(db)
            .await
            .unwrap();
        UserMediaTracker::mark_failure(db, auth.id, &MediaTrackerError::reauth("401"))
            .await
            .unwrap();

        let other = UserMediaTracker::new(bob, addon, creds("b"), vec![]);
        other
            .upsert(db)
            .await
            .unwrap();
        UserMediaTracker::mark_failure(
            db,
            other.id,
            &MediaTrackerError::permanent("400"),
        )
        .await
        .unwrap();

        assert_eq!(
            UserMediaTracker::get(db, auth.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            MediaTrackerStatus::AuthExpired
        );
        assert_eq!(
            UserMediaTracker::get(db, other.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            MediaTrackerStatus::Error
        );
    }

    #[tokio::test]
    async fn success_clears_a_previous_error() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;
        let row = UserMediaTracker::new(user, addon, creds("a"), vec![]);
        row.upsert(db)
            .await
            .unwrap();

        UserMediaTracker::mark_failure(db, row.id, &MediaTrackerError::reauth("401"))
            .await
            .unwrap();
        UserMediaTracker::mark_success(db, row.id)
            .await
            .unwrap();

        let got = UserMediaTracker::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, MediaTrackerStatus::Connected);
        assert!(
            got.last_error
                .is_none()
        );
        assert!(
            got.last_error_kind
                .is_none()
        );
        assert!(
            got.last_success_at
                .is_some()
        );
    }

    #[tokio::test]
    async fn filters_change_without_touching_credentials() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;
        let row = UserMediaTracker::new(
            user,
            addon,
            creds("secret"),
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        row.upsert(db)
            .await
            .unwrap();

        UserMediaTracker::set_event_filters(
            db,
            row.id,
            &[
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::Favorite,
            ],
        )
        .await
        .unwrap();

        let got = UserMediaTracker::get(db, row.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            got.event_filters,
            vec![
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::Favorite
            ]
        );
        assert_eq!(
            got.credentials
                .get_str("token"),
            Some("secret"),
            "changing filters must not require reconnecting"
        );
    }

    #[tokio::test]
    async fn deleting_a_user_takes_their_connections_with_them() {
        let (_srv, guard) = new_test_server()
            .await
            .unwrap();
        let db = &guard
            .0
            .db;
        let addon = seed_addon(db).await;
        let user = seed_user(db, "alice").await;
        UserMediaTracker::new(user, addon, creds("a"), vec![])
            .upsert(db)
            .await
            .unwrap();

        crate::db::User::delete(db, &user)
            .await
            .unwrap();

        assert!(
            UserMediaTracker::list_for_user(db, user)
                .await
                .unwrap()
                .is_empty(),
            "credentials must not outlive the account"
        );
    }

    #[tokio::test]
    async fn credentials_are_never_serialised_outward() {
        // The API returns this struct; the blob must not ride along.
        let row = UserMediaTracker::new(
            Uuid::from_u128(1),
            Uuid::from_u128(2),
            creds("super-secret"),
            vec![],
        );
        let json = serde_json::to_string(&row).unwrap();
        assert!(
            !json.contains("super-secret"),
            "credentials leaked into the wire format: {json}"
        );
    }
}
