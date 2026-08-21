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
    addons::media_tracker::{MediaTrackerEvent, MediaTrackerTarget},
    db,
};

fn describe(media: &db::Media, series: Option<&db::Media>) -> MediaTrackerTarget {
    MediaTrackerTarget {
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
) -> Result<Option<MediaTrackerTarget>> {
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
    event: MediaTrackerEvent,
) -> Result<usize> {
    if !ctx
        .addons
        .has_media_tracker()
    {
        return Ok(0);
    }

    let kind = event.kind();
    // Both filters have to hold: the user asked for it, and the provider said
    // it can take it. Queueing past either only produces permanent failures.
    let wanted: Vec<db::UserMediaTracker> =
        db::UserMediaTracker::list_for_user(&ctx.db, user_id)
            .await?
            .into_iter()
            .filter(|t| t.status == db::MediaTrackerStatus::Connected && t.wants(kind))
            .filter(|t| {
                ctx.addons
                    .media_tracker_for(t.addon_id)
                    .is_some_and(|a| {
                        a.capabilities()
                            .supports(kind)
                    })
            })
            .collect();
    if wanted.is_empty() {
        return Ok(0);
    }

    for tracker in &wanted {
        db::DeliveryQueue::new(db::QueueKind::MediaTracker {
            user_media_tracker_id: tracker.id,
            payload: db::MediaTrackerPayload {
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
    event: MediaTrackerEvent,
) {
    match enqueue(&state.ctx, user_id, media, event).await {
        Ok(0) => {}
        Ok(_) => {
            let _ = state
                .tasks
                .run_task(crate::tasks::DELIVERY_QUEUE_SYNC_KEY)
                .await;
        }
        Err(e) => warn!(error = %e, "could not queue media tracker delivery"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        addons::{
            Addon, AddonCapabilities, AddonKind, AddonPresetRef, AddonRuntime,
            media_tracker::{
                MediaTrackerAddon, MediaTrackerCapabilities, MediaTrackerCredentials,
                MediaTrackerCtx, MediaTrackerEventKind, MediaTrackerResult,
            },
        },
        db::{DeliveryQueue, MediaTrackerStatus, UserMediaTracker},
        integration_test::new_test_server,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;

    /// Enough of a provider for the registry to report that media tracking is
    /// installed. What it does with an event is the sync task's business.
    struct StubMediaTracker(Vec<MediaTrackerEventKind>);

    impl StubMediaTracker {
        /// Takes everything, so a test only names events when the point is
        /// that one of them is refused.
        fn everything() -> Self {
            Self(vec![
                MediaTrackerEventKind::PlaybackStart,
                MediaTrackerEventKind::PlaybackProgress,
                MediaTrackerEventKind::PlaybackStop,
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::MarkUnplayed,
                MediaTrackerEventKind::Favorite,
                MediaTrackerEventKind::Rating,
            ])
        }
    }

    impl AddonKind for StubMediaTracker {
        fn id(&self) -> &'static str {
            "scripted"
        }
    }

    #[async_trait]
    impl MediaTrackerAddon for StubMediaTracker {
        fn capabilities(&self) -> MediaTrackerCapabilities {
            MediaTrackerCapabilities {
                supported_events: self
                    .0
                    .clone(),
                ..Default::default()
            }
        }

        async fn on_event(
            &self,
            _event: &MediaTrackerEvent,
            _target: &MediaTrackerTarget,
            _creds: &MediaTrackerCredentials,
            _ctx: &MediaTrackerCtx,
        ) -> MediaTrackerResult<()> {
            Ok(())
        }
    }

    async fn connect(
        ctx: &AppContext,
        name: &str,
        status: MediaTrackerStatus,
        filters: Vec<MediaTrackerEventKind>,
    ) -> Uuid {
        connect_to(ctx, name, status, filters, StubMediaTracker::everything()).await
    }

    /// As `connect`, but against a provider that only declares some events.
    async fn connect_to(
        ctx: &AppContext,
        name: &str,
        status: MediaTrackerStatus,
        filters: Vec<MediaTrackerEventKind>,
        provider: StubMediaTracker,
    ) -> Uuid {
        let addon = crate::integration_test::register_media_tracker(
            ctx,
            name,
            Arc::new(provider),
        )
        .await;
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
            vec![MediaTrackerEventKind::MarkPlayed],
        )
        .await;
        let media = crate::integration_test::seed_episode(ctx).await;

        enqueue(
            ctx,
            user_id(ctx).await,
            &media,
            MediaTrackerEvent::MarkPlayed,
        )
        .await
        .unwrap();

        let rows = queued(ctx, tracker).await;
        assert_eq!(rows.len(), 1);
        let db::QueueKind::MediaTracker { payload, .. } = &rows[0].kind;
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
            vec![MediaTrackerEventKind::PlaybackStop],
        )
        .await;
        let played = connect(
            ctx,
            "played",
            MediaTrackerStatus::Connected,
            vec![MediaTrackerEventKind::MarkPlayed],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;

        let n = enqueue(
            ctx,
            user_id(ctx).await,
            &media,
            MediaTrackerEvent::MarkPlayed,
        )
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
    async fn an_event_the_provider_does_not_take_is_not_queued() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let tracker = connect_to(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::Rating,
            ],
            StubMediaTracker(vec![MediaTrackerEventKind::MarkPlayed]),
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;

        let n = enqueue(
            ctx,
            user_id(ctx).await,
            &media,
            MediaTrackerEvent::Rating { rating: Some(7.0) },
        )
        .await
        .unwrap();

        assert_eq!(
            n, 0,
            "delivering it would fail permanently and take the connection down"
        );
        assert!(
            queued(ctx, tracker)
                .await
                .is_empty()
        );
    }

    #[tokio::test]
    async fn nothing_is_queued_when_no_addon_can_track() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let tracker = connect(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![MediaTrackerEventKind::MarkPlayed],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;
        ctx.addons
            .replace_runtimes_for_test(Vec::new());

        let n = enqueue(
            ctx,
            user_id(ctx).await,
            &media,
            MediaTrackerEvent::MarkPlayed,
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "an uninstalled addon has nothing to deliver to");
        assert!(
            queued(ctx, tracker)
                .await
                .is_empty()
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
            vec![MediaTrackerEventKind::MarkPlayed],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;

        let n = enqueue(
            ctx,
            user_id(ctx).await,
            &media,
            MediaTrackerEvent::MarkPlayed,
        )
        .await
        .unwrap();

        assert_eq!(n, 0, "queueing for it would only pile up failures");
        assert!(
            queued(ctx, expired)
                .await
                .is_empty()
        );
    }

    /// A client may report a stop carrying no item id, which is why the
    /// handler falls back to the session's. The scrobble has to use the same
    /// fallback, or the finish is the one event that goes missing.
    #[tokio::test]
    async fn a_stop_without_an_item_id_still_queues_the_watch() {
        let (server, guard, token) =
            crate::integration_test::authenticated_server().await;
        let ctx = &guard.0;
        let tracker = connect(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![MediaTrackerEventKind::PlaybackStop],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;
        let auth = crate::integration_test::auth_header_with_token(&token);

        server
            .post("/sessions/playing")
            .add_header(
                http::header::AUTHORIZATION,
                http::header::HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&crate::api::PlaybackInfo {
                item_id: media.id,
                play_session_id: Some("ps1".into()),
                ..Default::default()
            })
            .await
            .assert_status_success();

        server
            .post("/sessions/playing/stopped")
            .add_header(
                http::header::AUTHORIZATION,
                http::header::HeaderValue::from_str(&auth).unwrap(),
            )
            .json(&crate::api::PlaybackInfo {
                play_session_id: Some("ps1".into()),
                position_ticks: Some(1),
                ..Default::default()
            })
            .await
            .assert_status_success();

        let rows = queued(ctx, tracker).await;
        assert_eq!(rows.len(), 1, "the session knew what was playing");
        let db::QueueKind::MediaTracker { payload, .. } = &rows[0].kind;
        assert_eq!(payload.media_id, media.id);
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
            vec![MediaTrackerEventKind::Favorite],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;
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
        let db::QueueKind::MediaTracker { payload, .. } = &rows[0].kind;
        assert_eq!(
            payload.event,
            MediaTrackerEvent::Favorite { is_favorite: true }
        );
    }

    /// Rates `movie` over the API and returns the events that were queued for
    /// it.
    async fn rate(path_suffix: &str, delete: bool) -> Vec<MediaTrackerEvent> {
        let (server, guard, token) =
            crate::integration_test::authenticated_server().await;
        let ctx = &guard.0;
        let tracker = connect(
            ctx,
            "a",
            MediaTrackerStatus::Connected,
            vec![MediaTrackerEventKind::Rating],
        )
        .await;
        let media = crate::integration_test::seed_movie(ctx).await;
        let url = format!("/useritems/{}/rating{path_suffix}", media.id);
        let header = (
            http::header::AUTHORIZATION,
            http::header::HeaderValue::from_str(
                &crate::integration_test::auth_header_with_token(&token),
            )
            .unwrap(),
        );

        if delete {
            server
                .delete(&url)
                .add_header(header.0, header.1)
                .await
                .assert_status_ok();
        } else {
            server
                .post(&url)
                .add_header(header.0, header.1)
                .await
                .assert_status_ok();
        }

        queued(ctx, tracker)
            .await
            .iter()
            .map(|row| {
                let db::QueueKind::MediaTracker { payload, .. } = &row.kind;
                payload
                    .event
                    .clone()
            })
            .collect()
    }

    #[tokio::test]
    async fn rating_an_item_queues_the_score() {
        assert_eq!(
            rate("?rating=7", false).await,
            vec![MediaTrackerEvent::Rating { rating: Some(7.0) }]
        );
    }

    #[tokio::test]
    async fn clearing_a_rating_queues_the_removal() {
        assert_eq!(
            rate("", true).await,
            vec![MediaTrackerEvent::Rating { rating: None }],
            "a cleared rating is itself the update a provider needs"
        );
    }
}
