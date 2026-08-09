use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Instant,
};
use tokio::sync::broadcast;
use tracing::{debug, warn};
use uuid::Uuid;

use crate::{AppContext, addons, db};

// ---------------------------------------------------------------------------
// TrackingEvent
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum TrackingEvent {
    PlaybackStart {
        user_id: Uuid,
        media_id: Uuid,
        session_id: String,
        position_ticks: i64,
    },
    PlaybackProgress {
        user_id: Uuid,
        media_id: Uuid,
        session_id: String,
        position_ticks: i64,
        is_paused: bool,
    },
    PlaybackStop {
        user_id: Uuid,
        media_id: Uuid,
        session_id: String,
        position_ticks: i64,
    },
    MarkPlayed {
        user_id: Uuid,
        media_id: Uuid,
    },
    MarkUnplayed {
        user_id: Uuid,
        media_id: Uuid,
    },
    Favorite {
        user_id: Uuid,
        media_id: Uuid,
        is_favorite: bool,
    },
    UserRating {
        user_id: Uuid,
        media_id: Uuid,
        rating: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackingEventKind {
    PlaybackStart,
    PlaybackProgress,
    PlaybackStop,
    MarkPlayed,
    MarkUnplayed,
    Favorite,
    UserRating,
}

impl TrackingEvent {
    pub fn user_id(&self) -> Uuid {
        match self {
            Self::PlaybackStart { user_id, .. }
            | Self::PlaybackProgress { user_id, .. }
            | Self::PlaybackStop { user_id, .. }
            | Self::MarkPlayed { user_id, .. }
            | Self::MarkUnplayed { user_id, .. }
            | Self::Favorite { user_id, .. }
            | Self::UserRating { user_id, .. } => *user_id,
        }
    }

    pub fn media_id(&self) -> Uuid {
        match self {
            Self::PlaybackStart { media_id, .. }
            | Self::PlaybackProgress { media_id, .. }
            | Self::PlaybackStop { media_id, .. }
            | Self::MarkPlayed { media_id, .. }
            | Self::MarkUnplayed { media_id, .. }
            | Self::Favorite { media_id, .. }
            | Self::UserRating { media_id, .. } => *media_id,
        }
    }

    pub fn kind(&self) -> TrackingEventKind {
        match self {
            Self::PlaybackStart { .. } => TrackingEventKind::PlaybackStart,
            Self::PlaybackProgress { .. } => TrackingEventKind::PlaybackProgress,
            Self::PlaybackStop { .. } => TrackingEventKind::PlaybackStop,
            Self::MarkPlayed { .. } => TrackingEventKind::MarkPlayed,
            Self::MarkUnplayed { .. } => TrackingEventKind::MarkUnplayed,
            Self::Favorite { .. } => TrackingEventKind::Favorite,
            Self::UserRating { .. } => TrackingEventKind::UserRating,
        }
    }
}

// ---------------------------------------------------------------------------
// Progress throttle — suppresses rapid progress events per session
// ---------------------------------------------------------------------------

#[derive(Clone, Default)]
pub struct ProgressThrottle {
    inner: Arc<Mutex<HashMap<String, Instant>>>,
}

impl ProgressThrottle {
    /// Returns true if the event should be forwarded, false if it should be dropped.
    /// Progress events for the same session are forwarded at most once per 60 seconds.
    pub fn should_emit(&self, session_id: &str) -> bool {
        let mut map = self
            .inner
            .lock()
            .unwrap();
        let now = Instant::now();
        match map.get(session_id) {
            Some(last)
                if now
                    .duration_since(*last)
                    .as_secs()
                    < 60 =>
            {
                false
            }
            _ => {
                map.insert(session_id.to_string(), now);
                true
            }
        }
    }

    pub fn remove(&self, session_id: &str) {
        self.inner
            .lock()
            .unwrap()
            .remove(session_id);
    }
}

// ---------------------------------------------------------------------------
// Background consumer
// ---------------------------------------------------------------------------

pub async fn run_consumer(mut rx: broadcast::Receiver<TrackingEvent>, ctx: AppContext) {
    loop {
        match rx
            .recv()
            .await
        {
            Ok(event) => {
                let ctx = ctx.clone();
                tokio::spawn(async move {
                    dispatch_event(event, ctx).await;
                });
            }
            Err(broadcast::error::RecvError::Lagged(n)) => {
                warn!(skipped = n, "tracking consumer lagged, events dropped");
            }
            Err(broadcast::error::RecvError::Closed) => break,
        }
    }
}

async fn dispatch_event(event: TrackingEvent, ctx: AppContext) {
    let user_id = event.user_id();
    let media_id = event.media_id();

    let user = match db::User::get_by_id(&ctx.db, &user_id).await {
        Ok(Some(u)) => u,
        _ => return,
    };
    let media = match db::Media::get_by_id(&ctx.db, &media_id).await {
        Ok(Some(m)) => m,
        _ => return,
    };

    let addons = ctx
        .addons
        .list_for_user(&ctx.db, Some(user_id))
        .await;
    for runtime in &addons {
        let Some(tracker) = runtime
            .caps
            .tracking
            .as_ref()
        else {
            continue;
        };

        // Filter by event kind if the addon declares a filter.
        if let Some(filter) = tracker.event_filter() {
            if !filter.contains(&event.kind()) {
                continue;
            }
        }

        let user_config = addons::addon::get_user_addon_config(
            &ctx.db,
            user_id,
            runtime
                .row
                .id,
        )
        .await
        .unwrap_or_default();

        debug!(
            addon = %runtime.row.name,
            event = ?event.kind(),
            "dispatching tracking event"
        );

        let tracking_ctx = addons::TrackingCtx {
            config: std::sync::Arc::new(
                ctx.config
                    .clone(),
            ),
            db: ctx
                .db
                .clone(),
        };

        if let Err(e) = tracker
            .on_event(&event, &user, &media, &user_config, &tracking_ctx)
            .await
        {
            warn!(
                error = %e,
                addon = %runtime.row.name,
                "tracking addon failed"
            );
        }
    }
}
