use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, warn};

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{
    AppContext,
    addons::tracking::{
        TrackingCtx, TrackingError, TrackingEvent, TrackingResult, TrackingTarget,
    },
    db,
};

/// How many due rows one pass claims. Bounded so a large backlog does not hold
/// the task open indefinitely; the next pass picks up the rest.
const BATCH: i64 = 200;

/// Delivered rows older than this are trimmed. Failed rows are kept, since
/// they are what the activity views read.
const KEEP_DELIVERED_DAYS: i64 = 7;

/// What one outbox row carries: the event plus the item it was about, resolved
/// at enqueue time so delivery never has to look anything up.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MediaTrackerOutboxPayload {
    pub event: TrackingEvent,
    pub target: TrackingTarget,
}

pub struct MediaTrackerSyncTask;

#[async_trait]
impl Task for MediaTrackerSyncTask {
    fn key(&self) -> &str {
        "MediaTrackerSync"
    }
    fn name(&self) -> &str {
        "Media Tracker Sync"
    }
    fn description(&self) -> &str {
        "Delivers pending watch activity to connected tracking services, retrying \
         transient failures with backoff."
    }
    fn short_description(&self) -> &str {
        "Delivers pending tracking activity"
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
        let purged =
            db::MediaTrackerOutbox::purge_delivered(&ctx.db, KEEP_DELIVERED_DAYS)
                .await
                .unwrap_or(0);
        debug!(delivered, purged, "tracking sync pass complete");
        progress.set(100.0);
        Ok(())
    }
}

/// Attempt every due row once. Returns how many were delivered.
///
/// Failures are recorded per row and never abort the pass: one dead provider
/// must not stop another user's scrobbles from going out.
pub async fn drain(ctx: &AppContext, progress: &ProgressReporter) -> Result<usize> {
    let due = db::MediaTrackerOutbox::due(&ctx.db, BATCH).await?;
    if due.is_empty() {
        return Ok(0);
    }

    let total = due.len() as f64;
    let mut delivered = 0usize;

    for (i, row) in due
        .into_iter()
        .enumerate()
    {
        match deliver(ctx, &row).await {
            Ok(()) => {
                db::MediaTrackerOutbox::mark_delivered(&ctx.db, row.id).await?;
                db::UserMediaTracker::mark_success(&ctx.db, row.user_media_tracker_id)
                    .await?;
                delivered += 1;
            }
            Err(err) => {
                let status = db::MediaTrackerOutbox::record_failure(
                    &ctx.db,
                    row.id,
                    row.attempts,
                    &err,
                )
                .await?;
                // A retryable blip is the worker's business, not the user's, so
                // only a terminal outcome touches the media tracker's health.
                if status != db::MediaTrackerOutboxStatus::Pending {
                    db::UserMediaTracker::mark_failure(
                        &ctx.db,
                        row.user_media_tracker_id,
                        &err,
                    )
                    .await?;
                }
                warn!(
                    outbox_id = %row.id,
                    attempts = row.attempts + 1,
                    ?status,
                    error = %err,
                    "tracking delivery failed"
                );
            }
        }
        progress.set((i as f64 + 1.0) / total * 100.0);
    }

    Ok(delivered)
}

async fn deliver(ctx: &AppContext, row: &db::MediaTrackerOutbox) -> TrackingResult<()> {
    let conn = db::UserMediaTracker::get(&ctx.db, row.user_media_tracker_id)
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

    let payload: MediaTrackerOutboxPayload = serde_json::from_str(&row.payload)
        .map_err(|e| TrackingError::permanent(format!("unreadable payload: {e}")))?;

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
