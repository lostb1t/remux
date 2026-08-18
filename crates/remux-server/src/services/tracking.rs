//! Queueing a user's actions for their media trackers, and describing the item
//! a delivery names. Describing it at delivery keeps the walk to the series off
//! the playback path. Nothing here talks to a provider; the sync task does that.

use anyhow::Result;
use chrono::Datelike;
use sqlx::SqlitePool;
use tracing::warn;
use uuid::Uuid;

use crate::{
    AppContext,
    addons::tracking::{TrackingEvent, TrackingTarget},
    db,
};

fn describe(media: &db::Media, series: Option<&db::Media>) -> TrackingTarget {
    TrackingTarget {
        kind: media
            .kind
            .clone(),
        title: media
            .title
            .clone(),
        year: media
            .released_at
            .map(|d| d.year()),
        ids: media
            .external_ids
            .clone(),
        series: series.map(|s| Box::new(describe(s, None))),
        season: media.parent_idx,
        episode: media.idx,
    }
}

/// The item as a provider needs to see it, or `None` when nothing about it
/// carries an id one could match on. Episodes carry their series, because a
/// provider keys an episode on the show's ids plus season and episode.
pub async fn resolve_target(
    db: &SqlitePool,
    media: &db::Media,
) -> Result<Option<TrackingTarget>> {
    let series = if media.kind == db::MediaKind::Episode {
        db::Media::get_ancestors(db, &media.id)
            .await?
            .into_iter()
            .find(|m| m.kind == db::MediaKind::Series)
    } else {
        None
    };

    let target = describe(media, series.as_ref());
    Ok(target
        .is_matchable()
        .then_some(target))
}

/// Queue `event` for every media tracker this user has connected that wants
/// it. Returns how many rows were written.
pub async fn enqueue(
    ctx: &AppContext,
    user_id: Uuid,
    media: &db::Media,
    event: TrackingEvent,
) -> Result<usize> {
    let wanted: Vec<db::UserMediaTracker> =
        db::UserMediaTracker::list_for_user(&ctx.db, user_id)
            .await?
            .into_iter()
            .filter(|t| {
                t.status == db::MediaTrackerStatus::Connected && t.wants(event.kind())
            })
            .collect();
    if wanted.is_empty() {
        return Ok(0);
    }

    for tracker in &wanted {
        db::DeliveryQueue::new(db::QueueKind::Tracker {
            user_media_tracker_id: tracker.id,
            payload: db::TrackerPayload {
                media_id: media.id,
                event: event.clone(),
            },
        })
        .insert(&ctx.db)
        .await?;
    }
    Ok(wanted.len())
}

/// Queue `event`, then nudge the worker so a scrobble does not wait for the
/// next sweep. Never fails the caller: the user's action already succeeded.
pub async fn enqueue_and_wake(
    state: &crate::AppState,
    user_id: Uuid,
    media: &db::Media,
    event: TrackingEvent,
) {
    match enqueue(&state.ctx, user_id, media, event).await {
        Ok(0) => {}
        Ok(_) => {
            let _ = state
                .tasks
                .run_task(crate::tasks::DELIVERY_QUEUE_SYNC_KEY)
                .await;
        }
        Err(e) => warn!(error = %e, "could not queue tracking delivery"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        addons::{Addon, AddonPresetRef, tracking::TrackingEventKind},
        db::{DeliveryQueue, MediaTrackerStatus, UserMediaTracker},
        integration_test::new_test_server,
    };
    use chrono::Utc;

    async fn connect(
        ctx: &AppContext,
        name: &str,
        status: MediaTrackerStatus,
        filters: Vec<TrackingEventKind>,
    ) -> Uuid {
        let now = Utc::now().naive_utc();
        let addon = Addon {
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
        };
        addon
            .insert(&ctx.db)
            .await
            .unwrap();

        let mut tracker = UserMediaTracker::new(
            user_id(ctx).await,
            addon.id,
            Default::default(),
            filters,
        );
        tracker.status = status;
        tracker
            .upsert(&ctx.db)
            .await
            .unwrap();
        tracker.id
    }

    /// The single user every tracker in these tests belongs to.
    async fn user_id(ctx: &AppContext) -> Uuid {
        db::User::get_by_username(&ctx.db, "test")
            .await
            .unwrap()
            .unwrap()
            .id
    }

    /// Movies and series carry a UUID derived from their external ids, so the
    /// id cannot be left to `Default`.
    fn stable_id(kind: db::MediaKind, external_ids: &db::ExternalIds) -> Uuid {
        Uuid::from(&db::MediaIdRaw {
            kind,
            external_ids: external_ids.clone(),
            season: None,
            episode: None,
        })
    }

    async fn movie(ctx: &AppContext) -> db::Media {
        let external_ids = db::ExternalIds {
            imdb: db::NonEmptyString::try_new("tt0113277".to_string()).ok(),
            tmdb: Some(949),
            ..Default::default()
        };
        let mut m = db::Media {
            id: stable_id(db::MediaKind::Movie, &external_ids),
            title: "Heat".into(),
            kind: db::MediaKind::Movie,
            external_ids,
            ..Default::default()
        };
        m.save(&ctx.db)
            .await
            .unwrap();
        m
    }

    async fn episode(ctx: &AppContext) -> db::Media {
        let external_ids = db::ExternalIds {
            imdb: db::NonEmptyString::try_new("tt0306414".to_string()).ok(),
            tvdb: Some(79126),
            ..Default::default()
        };
        let mut series = db::Media {
            id: stable_id(db::MediaKind::Series, &external_ids),
            title: "The Wire".into(),
            kind: db::MediaKind::Series,
            external_ids,
            ..Default::default()
        };
        series
            .save(&ctx.db)
            .await
            .unwrap();

        let mut season = db::Media {
            title: "Season 1".into(),
            kind: db::MediaKind::Season,
            parent_id: Some(series.id),
            grandparent_id: Some(series.id),
            idx: Some(1),
            ..Default::default()
        };
        season
            .save(&ctx.db)
            .await
            .unwrap();

        let mut ep = db::Media {
            title: "The Target".into(),
            kind: db::MediaKind::Episode,
            parent_id: Some(season.id),
            grandparent_id: Some(series.id),
            idx: Some(1),
            parent_idx: Some(1),
            ..Default::default()
        };
        ep.save(&ctx.db)
            .await
            .unwrap();
        ep
    }

    async fn queued(ctx: &AppContext, tracker: Uuid) -> Vec<DeliveryQueue> {
        DeliveryQueue::list_for_media_tracker(&ctx.db, tracker, 10)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_queued_row_names_the_item_rather_than_carrying_it() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let tracker = connect(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![TrackingEventKind::MarkPlayed],
        )
        .await;
        let media = episode(ctx).await;

        enqueue(ctx, user_id(ctx).await, &media, TrackingEvent::MarkPlayed)
            .await
            .unwrap();

        let rows = queued(ctx, tracker).await;
        assert_eq!(rows.len(), 1);
        let db::QueueKind::Tracker { payload, .. } = &rows[0].kind;
        assert_eq!(payload.media_id, media.id);
    }

    #[tokio::test]
    async fn each_tracker_only_gets_the_events_it_asked_for() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let stops = connect(
            ctx,
            "stops",
            MediaTrackerStatus::Connected,
            vec![TrackingEventKind::PlaybackStop],
        )
        .await;
        let played = connect(
            ctx,
            "played",
            MediaTrackerStatus::Connected,
            vec![TrackingEventKind::MarkPlayed],
        )
        .await;
        let media = movie(ctx).await;

        let n = enqueue(ctx, user_id(ctx).await, &media, TrackingEvent::MarkPlayed)
            .await
            .unwrap();

        assert_eq!(n, 1);
        assert!(
            queued(ctx, stops)
                .await
                .is_empty()
        );
        assert_eq!(
            queued(ctx, played)
                .await
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_tracker_that_is_not_connected_is_skipped() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let expired = connect(
            ctx,
            "expired",
            MediaTrackerStatus::AuthExpired,
            vec![TrackingEventKind::MarkPlayed],
        )
        .await;
        let media = movie(ctx).await;

        let n = enqueue(ctx, user_id(ctx).await, &media, TrackingEvent::MarkPlayed)
            .await
            .unwrap();

        assert_eq!(n, 0, "queueing for it would only pile up failures");
        assert!(
            queued(ctx, expired)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn favouriting_through_the_api_queues_a_delivery() {
        let (server, guard, token) =
            crate::integration_test::authenticated_server().await;
        let ctx = &guard.0;
        let tracker = connect(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![TrackingEventKind::Favorite],
        )
        .await;
        let media = movie(ctx).await;
        let user = user_id(ctx).await;

        server
            .post(&format!("/users/{user}/favoriteitems/{}", media.id))
            .add_header(
                http::header::AUTHORIZATION,
                http::header::HeaderValue::from_str(
                    &crate::integration_test::auth_header_with_token(&token),
                )
                .unwrap(),
            )
            .await
            .assert_status_ok();

        let rows = queued(ctx, tracker).await;
        assert_eq!(rows.len(), 1);
        let db::QueueKind::Tracker { payload, .. } = &rows[0].kind;
        assert_eq!(payload.event, TrackingEvent::Favorite { is_favorite: true });
    }
}
