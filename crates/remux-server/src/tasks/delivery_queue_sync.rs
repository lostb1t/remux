use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{
    AppContext,
    addons::tracking::{TrackingCtx, TrackingError, TrackingResult},
    db,
};
use uuid::Uuid;

/// How many due rows one pass claims. Bounded so a large backlog does not hold
/// the task open indefinitely; the next pass picks up the rest.
const BATCH: i64 = 200;

/// Delivered rows older than this are trimmed. Failed rows are kept, since
/// they are what the activity views read.
const KEEP_DELIVERED_DAYS: i64 = 7;

pub struct DeliveryQueueSyncTask;

#[async_trait]
impl Task for DeliveryQueueSyncTask {
    fn key(&self) -> &str {
        "DeliveryQueueSync"
    }
    fn name(&self) -> &str {
        "Delivery Queue Sync"
    }
    fn description(&self) -> &str {
        "Delivers queued outbound events, currently watch activity headed for \
         connected tracking services, retrying transient failures with backoff."
    }
    fn short_description(&self) -> &str {
        "Delivers queued outbound events"
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let delivered = drain(&ctx, &progress).await?;
        // Trimming is housekeeping: a failure here should be visible but must
        // not fail a pass that already delivered.
        let purged = match db::DeliveryQueue::purge_delivered(
            &ctx.db,
            KEEP_DELIVERED_DAYS,
        )
        .await
        {
            Ok(n) => n,
            Err(e) => {
                warn!(error = %e, "failed to trim delivered queue rows");
                0
            }
        };
        debug!(delivered, purged, "delivery queue pass complete");
        progress.set(100.0);
        Ok(())
    }
}

/// Attempt every due row once. Returns how many were delivered.
///
/// Failures are recorded per row and never abort the pass: one dead provider
/// must not stop another user's deliveries from going out.
pub async fn drain(ctx: &AppContext, progress: &ProgressReporter) -> Result<usize> {
    let due = db::DeliveryQueue::due(&ctx.db, BATCH).await?;
    if due.is_empty() {
        return Ok(0);
    }

    let total = due.len() as f64;
    let mut delivered = 0usize;

    for (i, row) in due
        .into_iter()
        .enumerate()
    {
        match deliver(ctx, &row.kind).await {
            Ok(()) => {
                db::DeliveryQueue::mark_delivered(&ctx.db, row.id).await?;
                record_outcome(ctx, &row.kind, None).await?;
                delivered += 1;
            }
            Err(err) => {
                let status = db::DeliveryQueue::record_failure(
                    &ctx.db,
                    row.id,
                    row.attempts,
                    &err,
                )
                .await?;
                // A retryable blip is the worker's business, not the user's, so
                // only a terminal outcome touches the deliverer's health.
                if status != db::DeliveryStatus::Pending {
                    record_outcome(ctx, &row.kind, Some(&err)).await?;
                }
                warn!(
                    delivery_id = %row.id,
                    kind = %row.kind.kind(),
                    attempts = row.attempts + 1,
                    ?status,
                    error = %err,
                    "delivery failed"
                );
            }
        }
        progress.set((i as f64 + 1.0) / total * 100.0);
    }

    Ok(delivered)
}

/// Hand one row to whatever its kind talks to. Adding a kind means adding an
/// arm here.
async fn deliver(ctx: &AppContext, kind: &db::QueueKind) -> TrackingResult<()> {
    match kind {
        db::QueueKind::Tracker {
            user_media_tracker_id,
            payload,
        } => deliver_tracker(ctx, *user_media_tracker_id, payload).await,
    }
}

/// Reflect a settled delivery onto whatever owns it, so the UI can show a
/// connection as healthy or broken. Only terminal outcomes reach here.
async fn record_outcome(
    ctx: &AppContext,
    kind: &db::QueueKind,
    err: Option<&TrackingError>,
) -> Result<()> {
    match kind {
        db::QueueKind::Tracker {
            user_media_tracker_id,
            ..
        } => match err {
            None => {
                db::UserMediaTracker::mark_success(&ctx.db, *user_media_tracker_id)
                    .await
            }
            Some(err) => {
                db::UserMediaTracker::mark_failure(&ctx.db, *user_media_tracker_id, err)
                    .await
            }
        },
    }
}

async fn deliver_tracker(
    ctx: &AppContext,
    user_media_tracker_id: Uuid,
    payload: &db::TrackerPayload,
) -> TrackingResult<()> {
    let conn = db::UserMediaTracker::get(&ctx.db, user_media_tracker_id)
        .await
        .map_err(|e| TrackingError::retryable(format!("loading media tracker: {e}")))?
        .ok_or_else(|| TrackingError::permanent("media tracker no longer exists"))?;

    let addon = ctx
        .addons
        .tracking_for(conn.addon_id)
        .ok_or_else(|| {
            // The addon was disabled or removed since the row was queued.
            TrackingError::permanent("tracking addon is not enabled")
        })?;

    let tctx = TrackingCtx {
        config: Arc::new(
            ctx.config
                .clone(),
        ),
    };
    addon
        .on_event(&payload.event, &payload.target, &conn.credentials, &tctx)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        addons::{
            AddonCapabilities, AddonKind, AddonPresetRef, AddonRuntime,
            tracking::{
                TrackingAddon, TrackingCapabilities, TrackingCredentials,
                TrackingEvent, TrackingEventKind, TrackingIds, TrackingTarget,
            },
        },
        db::{
            DeliveryQueue, DeliveryStatus, MediaTrackerStatus, QueueKind,
            TrackerPayload, UserMediaTracker,
        },
        integration_test::new_test_server,
    };
    use chrono::Utc;
    use sqlx::SqlitePool;
    use std::{
        collections::VecDeque,
        sync::{Mutex, atomic::AtomicU64},
    };
    use uuid::Uuid;

    /// A provider that answers with whatever the test scripted, and records
    /// what it was handed. Standing in for a real addon is the only way to
    /// exercise the delivery loop before one exists.
    struct ScriptedAddon {
        script: Mutex<VecDeque<TrackingResult<()>>>,
        seen: Mutex<Vec<(TrackingEventKind, String)>>,
    }

    impl ScriptedAddon {
        /// Runs out of script -> succeeds, so the common case needs no setup.
        fn new(script: Vec<TrackingResult<()>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                seen: Mutex::new(Vec::new()),
            })
        }

        fn seen(&self) -> Vec<(TrackingEventKind, String)> {
            self.seen
                .lock()
                .unwrap()
                .clone()
        }
    }

    impl AddonKind for ScriptedAddon {
        fn id(&self) -> &'static str {
            "scripted"
        }
    }

    #[async_trait]
    impl TrackingAddon for ScriptedAddon {
        fn capabilities(&self) -> TrackingCapabilities {
            TrackingCapabilities::default()
        }

        async fn on_event(
            &self,
            event: &TrackingEvent,
            target: &TrackingTarget,
            _creds: &TrackingCredentials,
            _ctx: &TrackingCtx,
        ) -> TrackingResult<()> {
            self.seen
                .lock()
                .unwrap()
                .push((
                    event.kind(),
                    target
                        .title
                        .clone(),
                ));
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    fn addon_row(name: &str) -> crate::addons::Addon {
        let now = Utc::now().naive_utc();
        crate::addons::Addon {
            id: crate::common::get_uuid(),
            name: name.into(),
            preset: AddonPresetRef {
                kind: "scripted".into(),
                config: serde_json::Value::Null.into(),
            },
            resources: vec![],
            types: vec![],
            enabled: true,
            priority: 0,
            created_at: now,
            updated_at: now,
            system: false,
            is_default: true,
            http_redirect_stream: false,
            service_filter: vec![],
        }
    }

    /// Installs `addon` as the tracking capability of a stored addon row and
    /// connects `user` to it. Returns the media tracker's id.
    async fn connect(
        ctx: &AppContext,
        user: &str,
        addon: Arc<ScriptedAddon>,
    ) -> (Uuid, crate::addons::Addon) {
        let row = addon_row(user);
        row.insert(&ctx.db)
            .await
            .unwrap();

        let mut u =
            crate::db::User::new_with_password(String::new(), user.into(), "pw", None)
                .unwrap();
        u.save(&ctx.db)
            .await
            .unwrap();

        let conn = UserMediaTracker::new(
            u.id,
            row.id,
            TrackingCredentials::new(serde_json::json!({ "token": "t" })),
            vec![TrackingEventKind::PlaybackStop],
        );
        conn.upsert(&ctx.db)
            .await
            .unwrap();

        let mut runtimes: Vec<AddonRuntime> = ctx
            .addons
            .list_for_user(&ctx.db, None)
            .await;
        runtimes.push(AddonRuntime {
            row: row.clone(),
            caps: AddonCapabilities {
                tracking: Some(addon),
                ..Default::default()
            },
        });
        ctx.addons
            .replace_runtimes_for_test(runtimes);

        (conn.id, row)
    }

    fn payload(title: &str) -> TrackerPayload {
        TrackerPayload {
            event: TrackingEvent::PlaybackStop {
                position_ticks: 42,
                played: true,
            },
            target: TrackingTarget {
                kind: crate::db::MediaKind::Movie,
                title: title.into(),
                year: Some(1999),
                ids: TrackingIds {
                    imdb: Some("tt0133093".into()),
                    ..Default::default()
                },
                series: None,
                season: None,
                episode: None,
            },
        }
    }

    async fn queue(db: &SqlitePool, conn: Uuid, title: &str) -> Uuid {
        let row = DeliveryQueue::new(QueueKind::Tracker {
            user_media_tracker_id: conn,
            payload: payload(title),
        });
        row.insert(db)
            .await
            .unwrap();
        row.id
    }

    fn reporter() -> ProgressReporter {
        ProgressReporter::new(Arc::new(AtomicU64::new(0)))
    }

    #[tokio::test]
    async fn a_delivered_row_reaches_the_provider_and_marks_the_tracker_healthy() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![]);
        let (conn, _) = connect(ctx, "alice", addon.clone()).await;
        let row = queue(&ctx.db, conn, "The Matrix").await;

        assert_eq!(
            drain(ctx, &reporter())
                .await
                .unwrap(),
            1
        );

        assert_eq!(
            addon.seen(),
            vec![(TrackingEventKind::PlaybackStop, "The Matrix".to_string())],
            "the payload must round-trip to the provider intact"
        );

        let got = DeliveryQueue::get(&ctx.db, row)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, DeliveryStatus::Delivered);
        assert!(
            got.delivered_at
                .is_some()
        );

        let tracker = UserMediaTracker::get(&ctx.db, conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(tracker.status, MediaTrackerStatus::Connected);
        assert!(
            tracker
                .last_success_at
                .is_some(),
            "a delivery is what the health indicator reads"
        );
    }

    #[tokio::test]
    async fn a_retryable_failure_leaves_the_tracker_health_untouched() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![Err(TrackingError::retryable("503"))]);
        let (conn, _) = connect(ctx, "bob", addon).await;
        let row = queue(&ctx.db, conn, "Heat").await;

        assert_eq!(
            drain(ctx, &reporter())
                .await
                .unwrap(),
            0
        );

        let got = DeliveryQueue::get(&ctx.db, row)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, DeliveryStatus::Pending);
        assert_eq!(got.attempts, 1);

        let tracker = UserMediaTracker::get(&ctx.db, conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tracker.status,
            MediaTrackerStatus::Connected,
            "a blip the worker will ride out is not the user's problem"
        );
        assert!(
            tracker
                .last_error
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_rejected_token_flips_the_tracker_to_auth_expired() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![Err(TrackingError::reauth("401"))]);
        let (conn, _) = connect(ctx, "carol", addon).await;
        let row = queue(&ctx.db, conn, "Ronin").await;

        drain(ctx, &reporter())
            .await
            .unwrap();

        let got = DeliveryQueue::get(&ctx.db, row)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(got.status, DeliveryStatus::FailedPermanent);

        let tracker = UserMediaTracker::get(&ctx.db, conn)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            tracker.status,
            MediaTrackerStatus::AuthExpired,
            "this is what makes the UI offer a reconnect instead of an error"
        );
        assert!(
            tracker
                .last_error
                .is_some()
        );
    }

    #[tokio::test]
    async fn an_addon_disabled_since_enqueue_fails_permanently() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![]);
        let (conn, mut row) = connect(ctx, "dave", addon.clone()).await;
        let queued = queue(&ctx.db, conn, "Collateral").await;

        // Admin turns the addon off between enqueue and delivery.
        row.enabled = false;
        ctx.addons
            .replace_runtimes_for_test(vec![AddonRuntime {
                row,
                caps: AddonCapabilities {
                    tracking: Some(addon.clone()),
                    ..Default::default()
                },
            }]);

        drain(ctx, &reporter())
            .await
            .unwrap();

        assert!(
            addon
                .seen()
                .is_empty(),
            "must not deliver through an addon the admin has turned off"
        );
        let got = DeliveryQueue::get(&ctx.db, queued)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            got.status,
            DeliveryStatus::FailedPermanent,
            "retrying cannot re-enable an addon, so this must not sit pending forever"
        );
    }

    #[tokio::test]
    async fn one_failing_provider_does_not_hold_up_another() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let broken = ScriptedAddon::new(vec![Err(TrackingError::retryable("down"))]);
        let working = ScriptedAddon::new(vec![]);
        let (a, _) = connect(ctx, "erin", broken).await;
        let (b, _) = connect(ctx, "frank", working.clone()).await;
        let first = queue(&ctx.db, a, "Sicario").await;
        let second = queue(&ctx.db, b, "Arrival").await;

        assert_eq!(
            drain(ctx, &reporter())
                .await
                .unwrap(),
            1,
            "the pass must continue past a failure"
        );

        assert_eq!(
            DeliveryQueue::get(&ctx.db, first)
                .await
                .unwrap()
                .unwrap()
                .status,
            DeliveryStatus::Pending
        );
        assert_eq!(
            DeliveryQueue::get(&ctx.db, second)
                .await
                .unwrap()
                .unwrap()
                .status,
            DeliveryStatus::Delivered
        );
        assert_eq!(
            working
                .seen()
                .len(),
            1
        );
    }
}
