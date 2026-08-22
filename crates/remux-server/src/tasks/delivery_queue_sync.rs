use anyhow::Result;
use async_trait::async_trait;
use futures::StreamExt;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};
use tracing::{debug, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{
    AppContext,
    addons::media_tracker::{MediaTrackerCtx, MediaTrackerError, MediaTrackerResult},
    db,
};
use uuid::Uuid;

/// How many due rows one pass claims. Bounded so a large backlog does not hold
/// the task open indefinitely; the next pass picks up the rest.
const BATCH: i64 = 200;

/// Delivered rows older than this are trimmed. Failed rows are kept, since
/// they are what the activity views read.
const KEEP_DELIVERED_DAYS: i64 = 7;

/// Also the lookup key for waking the worker after an enqueue.
pub const DELIVERY_QUEUE_SYNC_KEY: &str = "DeliveryQueueSync";

pub struct DeliveryQueueSyncTask;

#[async_trait]
impl Task for DeliveryQueueSyncTask {
    fn key(&self) -> &str {
        DELIVERY_QUEUE_SYNC_KEY
    }
    fn name(&self) -> &str {
        "Delivery Queue Sync"
    }
    fn description(&self) -> &str {
        "Delivers queued outbound events, currently watch activity headed for \
         connected media trackers, retrying transient failures with backoff."
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
/// must not stop another user's deliveries from going out. Trackers are drained
/// concurrently, bounded by `delivery_concurrency`, because a row costs a round
/// trip to whatever it is delivered to and a provider that answers slowly
/// should hold up nobody but its own.
pub async fn drain(ctx: &AppContext, progress: &ProgressReporter) -> Result<usize> {
    let due = db::DeliveryQueue::due(&ctx.db, BATCH).await?;
    if due.is_empty() {
        return Ok(0);
    }

    let total = due.len() as f64;
    let concurrency = db::Settings::get_config_or_default(&ctx.db)
        .await
        .delivery_concurrency
        .max(1) as usize;

    // A queue per tracker, each holding its rows in the order `due` returned
    // them. Two rows for one tracker have to settle in order, a stop after the
    // start it follows, and one at a time, since their outcomes move the same
    // health status. Grouping is what guarantees that. Fanning every row out
    // and serializing after the fact would leave the order resting on how the
    // executor happens to poll, which is not something to build on.
    let mut by_tracker: Vec<(Uuid, Vec<db::DeliveryQueue>)> = Vec::new();
    for row in due {
        let db::QueueKind::MediaTracker {
            user_media_tracker_id,
            ..
        } = &row.kind;
        let tracker = *user_media_tracker_id;
        match by_tracker
            .iter_mut()
            .find(|(id, _)| *id == tracker)
        {
            Some((_, rows)) => rows.push(row),
            None => by_tracker.push((tracker, vec![row])),
        }
    }

    let delivered = Arc::new(AtomicUsize::new(0));
    let processed = Arc::new(AtomicUsize::new(0));

    futures::stream::iter(by_tracker)
        .map(|(_, rows)| {
            let ctx = ctx.clone();
            let progress = progress.clone();
            let delivered = Arc::clone(&delivered);
            let processed = Arc::clone(&processed);
            async move {
                for row in rows {
                    if process_row(&ctx, &row).await {
                        delivered.fetch_add(1, Ordering::Relaxed);
                    }
                    let n = processed.fetch_add(1, Ordering::Relaxed) + 1;
                    progress.set(n as f64 / total * 100.0);
                }
            }
        })
        .buffer_unordered(concurrency)
        .for_each(|()| async {})
        .await;

    Ok(delivered.load(Ordering::Relaxed))
}

/// One row through to a settled outcome. Returns whether it was delivered.
///
/// A failure recording the outcome is logged rather than propagated: it's one
/// row's bookkeeping, not a reason to give up on the rest of the pass.
async fn process_row(ctx: &AppContext, row: &db::DeliveryQueue) -> bool {
    match deliver(ctx, &row.kind).await {
        Ok(()) => {
            if let Err(e) = db::DeliveryQueue::mark_delivered(&ctx.db, row.id).await {
                warn!(delivery_id = %row.id, error = %e, "failed to mark delivery delivered");
            }
            if let Err(e) = record_outcome(ctx, &row.kind, None).await {
                warn!(delivery_id = %row.id, error = %e, "failed to record delivery outcome");
            }
            true
        }
        Err(err) => {
            let status = match db::DeliveryQueue::record_failure(
                &ctx.db,
                row.id,
                row.attempts,
                &err,
            )
            .await
            {
                Ok(status) => status,
                Err(e) => {
                    warn!(delivery_id = %row.id, error = %e, "failed to record delivery failure");
                    return false;
                }
            };
            // A retryable blip is the worker's business, not the user's, so
            // only a terminal outcome touches the deliverer's health.
            if status != db::DeliveryStatus::Pending {
                if let Err(e) = record_outcome(ctx, &row.kind, Some(&err)).await {
                    warn!(delivery_id = %row.id, error = %e, "failed to record delivery outcome");
                }
            }
            warn!(
                delivery_id = %row.id,
                kind = %row.kind.kind(),
                attempts = row.attempts + 1,
                ?status,
                error = %err,
                "delivery failed"
            );
            false
        }
    }
}

/// Hand one row to whatever its kind talks to. Adding a kind means adding an
/// arm here.
async fn deliver(ctx: &AppContext, kind: &db::QueueKind) -> MediaTrackerResult<()> {
    match kind {
        db::QueueKind::MediaTracker {
            user_media_tracker_id,
            payload,
        } => deliver_media_tracker(ctx, *user_media_tracker_id, payload).await,
    }
}

/// Reflect a settled delivery onto whatever owns it, so the UI can show a
/// connection as healthy or broken. Only terminal outcomes reach here.
async fn record_outcome(
    ctx: &AppContext,
    kind: &db::QueueKind,
    err: Option<&MediaTrackerError>,
) -> Result<()> {
    match kind {
        db::QueueKind::MediaTracker {
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

async fn deliver_media_tracker(
    ctx: &AppContext,
    user_media_tracker_id: Uuid,
    payload: &db::MediaTrackerPayload,
) -> MediaTrackerResult<()> {
    let conn = db::UserMediaTracker::get(&ctx.db, user_media_tracker_id)
        .await
        .map_err(|e| {
            MediaTrackerError::retryable(format!("loading media tracker: {e}"))
        })?
        .ok_or_else(|| {
            MediaTrackerError::permanent("media tracker no longer exists")
        })?;

    let addon = ctx
        .addons
        .media_tracker_for(conn.addon_id)
        .ok_or_else(|| {
            // The addon was disabled or removed since the row was queued.
            MediaTrackerError::permanent("media tracker addon is not enabled")
        })?;

    let media = db::Media::get_by_id(&ctx.db, &payload.media_id)
        .await
        .map_err(|e| MediaTrackerError::retryable(format!("loading item: {e}")))?
        .ok_or_else(|| MediaTrackerError::permanent("item no longer exists"))?;

    let target = crate::services::media_tracker::resolve_target(&ctx.db, &media)
        .await
        .map_err(|e| MediaTrackerError::retryable(format!("describing item: {e}")))?
        .ok_or_else(|| {
            MediaTrackerError::permanent("no external id a media tracker could match")
        })?;

    let tctx = MediaTrackerCtx {
        config: Arc::new(
            ctx.config
                .clone(),
        ),
    };
    addon
        .on_event(&payload.event, &target, &conn.credentials, &tctx)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        addons::{
            AddonCapabilities, AddonKind, AddonPresetRef, AddonRuntime,
            media_tracker::{
                MediaTrackerAddon, MediaTrackerCapabilities, MediaTrackerCredentials,
                MediaTrackerEvent, MediaTrackerEventKind, MediaTrackerTarget,
            },
        },
        db::{
            DeliveryQueue, DeliveryStatus, MediaTrackerPayload, MediaTrackerStatus,
            QueueKind, UserMediaTracker,
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
        script: Mutex<VecDeque<MediaTrackerResult<()>>>,
        seen: Mutex<Vec<(MediaTrackerEventKind, String)>>,
        targets: Mutex<Vec<MediaTrackerTarget>>,
    }

    impl ScriptedAddon {
        /// Runs out of script -> succeeds, so the common case needs no setup.
        fn new(script: Vec<MediaTrackerResult<()>>) -> Arc<Self> {
            Arc::new(Self {
                script: Mutex::new(script.into()),
                seen: Mutex::new(Vec::new()),
                targets: Mutex::new(Vec::new()),
            })
        }

        fn seen(&self) -> Vec<(MediaTrackerEventKind, String)> {
            self.seen
                .lock()
                .unwrap()
                .clone()
        }

        /// The items as they reached the provider.
        fn targets(&self) -> Vec<MediaTrackerTarget> {
            self.targets
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
    impl MediaTrackerAddon for ScriptedAddon {
        fn capabilities(&self) -> MediaTrackerCapabilities {
            MediaTrackerCapabilities::default()
        }

        async fn on_event(
            &self,
            event: &MediaTrackerEvent,
            target: &MediaTrackerTarget,
            _creds: &MediaTrackerCredentials,
            _ctx: &MediaTrackerCtx,
        ) -> MediaTrackerResult<()> {
            self.seen
                .lock()
                .unwrap()
                .push((
                    event.kind(),
                    target
                        .title
                        .clone(),
                ));
            self.targets
                .lock()
                .unwrap()
                .push(target.clone());
            self.script
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()))
        }
    }

    async fn connect(
        ctx: &AppContext,
        user: &str,
        addon: Arc<ScriptedAddon>,
    ) -> (Uuid, crate::addons::Addon) {
        let row =
            crate::integration_test::register_media_tracker(ctx, user, addon).await;

        let mut u =
            crate::db::User::new_with_password(String::new(), user.into(), "pw", None)
                .unwrap();
        u.save(&ctx.db)
            .await
            .unwrap();

        let conn = UserMediaTracker::new(
            u.id,
            row.id,
            MediaTrackerCredentials::new(serde_json::json!({ "token": "t" })),
            vec![MediaTrackerEventKind::PlaybackStop],
        );
        conn.upsert(&ctx.db)
            .await
            .unwrap();
        (conn.id, row)
    }

    /// A delivery names an item, so one has to exist. `imdb` is what makes it
    /// matchable.
    async fn movie(db: &SqlitePool, title: &str, imdb: &str) -> Uuid {
        let external_ids = crate::db::ExternalIds {
            imdb: crate::db::NonEmptyString::try_new(imdb.to_string()).ok(),
            ..Default::default()
        };
        let mut media = crate::db::Media {
            id: Uuid::from(&crate::db::MediaIdRaw {
                kind: crate::db::MediaKind::Movie,
                external_ids: external_ids.clone(),
                season: None,
                episode: None,
            }),
            title: title.into(),
            kind: crate::db::MediaKind::Movie,
            external_ids,
            ..Default::default()
        };
        media
            .save(db)
            .await
            .unwrap();
        media.id
    }

    async fn queue(db: &SqlitePool, conn: Uuid, title: &str) -> Uuid {
        queue_item(db, conn, movie(db, title, "tt0133093").await).await
    }

    async fn queue_item(db: &SqlitePool, conn: Uuid, media_id: Uuid) -> Uuid {
        let row = DeliveryQueue::new(QueueKind::MediaTracker {
            user_media_tracker_id: conn,
            payload: MediaTrackerPayload {
                media_id,
                event: MediaTrackerEvent::PlaybackStop {
                    position_ticks: 42,
                    played: true,
                },
            },
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
            vec![(
                MediaTrackerEventKind::PlaybackStop,
                "The Matrix".to_string()
            )],
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
        let addon = ScriptedAddon::new(vec![Err(MediaTrackerError::retryable("503"))]);
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
        let addon = ScriptedAddon::new(vec![Err(MediaTrackerError::reauth("401"))]);
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
                    media_tracker: Some(addon.clone()),
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

    /// Providers key an episode on its show, so the delivery path owes the
    /// series to every one of them.
    #[tokio::test]
    async fn an_episode_reaches_the_provider_with_its_series_attached() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![]);
        let (conn, _) = connect(ctx, "hana", addon.clone()).await;

        let episode = crate::integration_test::seed_episode(ctx).await;

        queue_item(&ctx.db, conn, episode.id).await;
        drain(ctx, &reporter())
            .await
            .unwrap();

        let targets = addon.targets();
        assert_eq!(targets.len(), 1);
        let target = &targets[0];
        assert_eq!(target.season, Some(1));
        assert_eq!(target.episode, Some(1));
        assert_eq!(
            target
                .series
                .as_ref()
                .expect("an episode alone identifies nothing")
                .ids
                .tvdb,
            Some(79126)
        );
    }

    #[tokio::test]
    async fn an_unmatchable_item_fails_the_row_rather_than_retrying_forever() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![]);
        let (conn, _) = connect(ctx, "gina", addon.clone()).await;
        let media = crate::integration_test::insert_test_source(ctx).await;
        let queued = queue_item(&ctx.db, conn, media.id).await;

        drain(ctx, &reporter())
            .await
            .unwrap();

        assert!(
            addon
                .seen()
                .is_empty(),
            "there is no id to hand a provider"
        );
        assert_eq!(
            DeliveryQueue::get(&ctx.db, queued)
                .await
                .unwrap()
                .unwrap()
                .status,
            DeliveryStatus::FailedPermanent,
            "no amount of retrying gives the item an id"
        );
    }

    /// Two events about one tracker have to reach it in the order they were
    /// queued: a stop arriving before the start it followed reads as a rewind,
    /// and both move the same health status. Draining groups rows by tracker
    /// for exactly that, so this is what would break if it stopped.
    #[tokio::test]
    async fn one_trackers_rows_go_out_in_the_order_they_were_queued() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon = ScriptedAddon::new(vec![]);
        let (conn, _) = connect(ctx, "gita", addon.clone()).await;

        // Own imdb id each, or they would be one media row under three names,
        // and distinct due times, so `due` has one legal order to preserve.
        let now = Utc::now().naive_utc();
        let titles = ["Dune", "Sicario", "Arrival"];
        for (i, title) in titles
            .iter()
            .enumerate()
        {
            let media = movie(&ctx.db, title, &format!("tt000000{i}")).await;
            let row = queue_item(&ctx.db, conn, media).await;
            sqlx::query("UPDATE delivery_queue SET next_attempt_at = ?1 WHERE id = ?2")
                .bind(now - chrono::Duration::seconds((titles.len() - i) as i64))
                .bind(row)
                .execute(&ctx.db)
                .await
                .unwrap();
        }

        assert_eq!(
            drain(ctx, &reporter())
                .await
                .unwrap(),
            3
        );
        assert_eq!(
            addon
                .seen()
                .into_iter()
                .map(|(_, title)| title)
                .collect::<Vec<_>>(),
            titles
        );
    }

    #[tokio::test]
    async fn one_failing_provider_does_not_hold_up_another() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let broken =
            ScriptedAddon::new(vec![Err(MediaTrackerError::retryable("down"))]);
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
