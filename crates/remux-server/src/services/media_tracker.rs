//! Describing the item a delivery names, and the subscriber that scrobbles it.

use std::{collections::HashMap, sync::Arc};

use anyhow::{Error, Result};
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use tracing::{info, warn};
use uuid::Uuid;

use crate::{
    AppContext,
    addons::media_tracker::{
        MediaTrackerCtx, MediaTrackerEvent, MediaTrackerTarget, RemoteProgress,
        RemoteWatch,
    },
    db,
    signals::{DeliveryMode, Event, EventType, Subscriber},
};

#[derive(Debug, Clone, Copy, Default)]
pub struct ImportStats {
    pub fetched: i64,
    pub matched: i64,
    pub updated: i64,
    pub deferred: i64,
    pub skipped: i64,
}

fn watch_identity(watch: &RemoteWatch) -> Option<(String, db::MediaIdRaw)> {
    let raw = db::MediaIdRaw {
        kind: if watch
            .season
            .is_some()
            && watch
                .episode
                .is_some()
        {
            db::MediaKind::Episode
        } else {
            db::MediaKind::Movie
        },
        external_ids: watch
            .ids
            .clone(),
        season: watch.season,
        episode: watch.episode,
    };
    raw.identity_key()
        .map(|key| (key, raw))
}

fn merge_watch(existing: &mut RemoteWatch, incoming: RemoteWatch) {
    existing.watched |= incoming.watched;
    existing.play_count = existing
        .play_count
        .max(incoming.play_count);
    existing.watched_at = existing
        .watched_at
        .max(incoming.watched_at);
    if incoming.progress_at >= existing.progress_at {
        if incoming
            .progress
            .is_some()
        {
            existing.progress = incoming.progress;
            existing.progress_at = incoming.progress_at;
        }
    }
    existing.favorite = incoming
        .favorite
        .or(existing.favorite);
    existing.rating = incoming
        .rating
        .or(existing.rating);
}

async fn find_by_ids(
    ctx: &AppContext,
    kind: db::MediaKind,
    ids: &db::ExternalIds,
) -> Result<Option<db::Media>> {
    Ok(sqlx::query_as::<_, db::Media>(
        "SELECT * FROM media WHERE kind = ?1 AND (\
         (?2 IS NOT NULL AND json_extract(external_ids, '$.imdb') = ?2) OR \
         (?3 IS NOT NULL AND json_extract(external_ids, '$.tmdb') = ?3) OR \
         (?4 IS NOT NULL AND json_extract(external_ids, '$.tvdb') = ?4)) LIMIT 1",
    )
    .bind(kind)
    .bind(
        ids.imdb
            .as_deref(),
    )
    .bind(ids.tmdb)
    .bind(ids.tvdb)
    .fetch_optional(&ctx.db)
    .await?)
}

async fn find_media_for_watch(
    ctx: &AppContext,
    watch: &RemoteWatch,
) -> Result<Option<db::Media>> {
    if watch
        .season
        .is_none()
        || watch
            .episode
            .is_none()
    {
        return find_by_ids(ctx, db::MediaKind::Movie, &watch.ids).await;
    }
    if let Some(episode) = find_by_ids(ctx, db::MediaKind::Episode, &watch.ids).await? {
        return Ok(Some(episode));
    }
    let Some(series) = find_by_ids(ctx, db::MediaKind::Series, &watch.ids).await?
    else {
        return Ok(None);
    };
    Ok(sqlx::query_as::<_, db::Media>(
        "SELECT * FROM media WHERE kind = 'episode' AND grandparent_id = ?1 \
         AND parent_idx = ?2 AND idx = ?3 LIMIT 1",
    )
    .bind(series.id)
    .bind(watch.season)
    .bind(watch.episode)
    .fetch_optional(&ctx.db)
    .await?)
}

fn progress_seconds(
    progress: RemoteProgress,
    runtime_seconds: Option<i64>,
) -> Option<i64> {
    match progress {
        RemoteProgress::Ticks(ticks) => Some(ticks.max(0) / 10_000_000),
        RemoteProgress::Percent(percent) => runtime_seconds.map(|seconds| {
            (seconds.max(0) as f64 * (percent.clamp(0.0, 100.0) as f64 / 100.0)) as i64
        }),
    }
}

/// Pull a complete provider snapshot and merge it into Remux without emitting
/// user-data signals, so an import can never echo back to the provider.
pub async fn import_tracker_history(
    ctx: &AppContext,
    tracker_id: Uuid,
) -> Result<ImportStats> {
    let mut tracker = db::UserMediaTracker::get(&ctx.db, tracker_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("media tracker connection not found"))?;
    let addon = ctx
        .addons
        .media_tracker_for(tracker.addon_id)
        .ok_or_else(|| anyhow::anyhow!("media tracker provider is unavailable"))?;
    let tctx = MediaTrackerCtx {
        config: Arc::new(
            ctx.config
                .clone(),
        ),
    };
    let mut watches_result = addon
        .import_history(&tracker.credentials, &tctx)
        .await;
    if watches_result
        .as_ref()
        .is_err_and(|error| error.requires_reauth())
    {
        match addon
            .refresh(&tracker.credentials, &tctx)
            .await
        {
            Ok(credentials) => {
                db::UserMediaTracker::replace_credentials(
                    &ctx.db,
                    tracker.id,
                    &credentials,
                )
                .await?;
                tracker.credentials = credentials;
                watches_result = addon
                    .import_history(&tracker.credentials, &tctx)
                    .await;
            }
            Err(refresh_error) => watches_result = Err(refresh_error),
        }
    }
    let watches = match watches_result {
        Ok(watches) => watches,
        Err(error) => {
            db::UserMediaTracker::mark_failure(&ctx.db, tracker.id, &error).await?;
            return Err(anyhow::anyhow!(error.to_string()));
        }
    };
    let mut stats = ImportStats {
        fetched: watches.len() as i64,
        ..Default::default()
    };
    let mut grouped: HashMap<String, (db::MediaIdRaw, RemoteWatch)> = HashMap::new();
    for watch in watches {
        let Some((key, raw)) = watch_identity(&watch) else {
            stats.skipped += 1;
            continue;
        };
        match grouped.get_mut(&key) {
            Some((_, existing)) => merge_watch(existing, watch),
            None => {
                grouped.insert(key, (raw, watch));
            }
        }
    }

    let user = db::User::get_by_id(&ctx.db, &tracker.user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("tracker user not found"))?;
    for (identity, (raw, watch)) in grouped {
        let media = find_media_for_watch(ctx, &watch).await?;
        let runtime = media
            .as_ref()
            .and_then(|media| media.runtime);
        let mut state = if let Some(media) = media.as_ref() {
            stats.matched += 1;
            db::UserMediaState::get_or_new(&ctx.db, &user, media).await?
        } else {
            stats.deferred += 1;
            let media_id = Uuid::from(&raw);
            sqlx::query_as::<_, db::UserMediaState>(
                "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
            )
            .bind(user.id)
            .bind(media_id)
            .fetch_optional(&ctx.db)
            .await?
            .unwrap_or(db::UserMediaState {
                user_id: user.id,
                media_id,
                media_raw: Some(identity),
                ..Default::default()
            })
        };

        let before = (
            state.play_count,
            state.played_at,
            state.last_played_at,
            state.playback_position,
        );
        let incoming_count = watch
            .play_count
            .unwrap_or(if watch.watched { 1 } else { 0 });
        if incoming_count > state.play_count {
            state.play_count = incoming_count;
        }
        if watch.watched && state.play_count == 0 {
            state.play_count = 1;
        }
        if let Some(watched_at) = watch.watched_at {
            state.played_at = state
                .played_at
                .max(Some(watched_at));
            state.last_played_at = state
                .last_played_at
                .max(Some(watched_at));
        }
        if watch.watched && watch.progress_at <= watch.watched_at {
            state.playback_position = 0;
        }
        if let (Some(progress), Some(progress_at)) = (watch.progress, watch.progress_at)
        {
            if state
                .last_played_at
                .is_none_or(|local| local <= progress_at)
            {
                if let Some(seconds) = progress_seconds(progress, runtime) {
                    state.playback_position = seconds;
                    state.last_played_at = Some(progress_at);
                }
            }
        }
        let after = (
            state.play_count,
            state.played_at,
            state.last_played_at,
            state.playback_position,
        );
        if after != before {
            state
                .save(&ctx.db)
                .await?;
            stats.updated += 1;
        } else if state
            .media_raw
            .is_some()
        {
            // Persist a new identity-only row even when all remote values are
            // zero; an existing row needs no write.
            let exists: i64 = sqlx::query_scalar(
                "SELECT COUNT(*) FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
            )
            .bind(state.user_id)
            .bind(state.media_id)
            .fetch_one(&ctx.db)
            .await?;
            if exists == 0 {
                state
                    .save(&ctx.db)
                    .await?;
            }
        }
    }
    db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
    Ok(stats)
}

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
        runtime_ticks: media
            .runtime
            .map(|seconds| seconds.saturating_mul(10_000_000)),
    }
}

/// The series an episode hangs off. The ancestor walk follows `parent_id`, so
/// an episode saved without a season row above it falls back to the
/// `grandparent_id` its row names, which `Media::validate` requires of it.
async fn series_of(ctx: &AppContext, media: &db::Media) -> Result<Option<db::Media>> {
    if media.kind != db::MediaKind::Episode {
        return Ok(None);
    }
    if let Some(series) = db::Media::get_ancestors(&ctx.db, &media.id)
        .await?
        .into_iter()
        .find(|m| m.kind == db::MediaKind::Series)
    {
        return Ok(Some(series));
    }
    let Some(series_id) = media.grandparent_id else {
        return Ok(None);
    };
    Ok(db::Media::get_by_id(&ctx.db, &series_id)
        .await?
        .filter(|m| m.kind == db::MediaKind::Series))
}

/// The item as a provider needs to see it, or `None` when nothing about it
/// carries an id one could match on. Episodes carry their series, because a
/// provider keys an episode on the show's ids plus season and episode.
pub async fn resolve_target(
    ctx: &AppContext,
    media: &mut db::Media,
) -> Result<Option<MediaTrackerTarget>> {
    let mut series = series_of(ctx, media).await?;

    // Opportunistic, and here so it reuses the series row loaded above: a TMDB
    // or Kitsu error must not hold up an event that was already deliverable, so
    // it is surfaced below only if it turns out to be why nothing matched.
    let mut completion_err: Option<Error> = None;
    if let Some(series) = series.as_mut() {
        if let Err(e) = crate::services::MediaResolveService::complete_episode_ids(
            media, series, ctx,
        )
        .await
        {
            completion_err = Some(e);
        }
    }

    let target = describe(media, series.as_ref());
    if !target.is_matchable() {
        if let Some(e) = completion_err {
            return Err(e);
        }
        return Ok(None);
    }
    if let Some(e) = completion_err {
        warn!(
            title = %media.title,
            error = %e,
            "failed to complete episode ids, delivering with what already matched"
        );
    }
    Ok(Some(target))
}

pub struct MediaTrackerSubscriber {
    pub ctx: AppContext,
}

#[async_trait]
impl Subscriber for MediaTrackerSubscriber {
    fn key(&self) -> &'static str {
        "media_tracker"
    }

    fn events(&self) -> &[EventType] {
        &[
            EventType::PlaybackStarted,
            EventType::PlaybackProgress,
            EventType::PlaybackStopped,
            EventType::MarkPlayed,
            EventType::MarkUnplayed,
            EventType::MarkFavorite,
            EventType::UnmarkFavorite,
            EventType::Rating,
        ]
    }

    fn delivery_mode(&self) -> DeliveryMode {
        // Persistence and retries live in `media_tracker_outbox`; retrying this
        // handler in memory could enqueue the same user action twice.
        DeliveryMode::Transient
    }

    async fn handle(&self, event: Event) -> anyhow::Result<()> {
        let (user_id, media_id, tracker_event) = match event {
            Event::PlaybackStarted(i) => (
                i.user_id,
                i.media_id,
                MediaTrackerEvent::PlaybackStart {
                    position_ticks: i.position_ticks,
                    session_id: i.session_id,
                },
            ),
            Event::PlaybackProgress(i) => (
                i.user_id,
                i.media_id,
                MediaTrackerEvent::PlaybackProgress {
                    position_ticks: i.position_ticks,
                    is_paused: i.is_paused,
                    session_id: i.session_id,
                },
            ),
            Event::PlaybackStopped(i) => (
                i.user_id,
                i.media_id,
                MediaTrackerEvent::PlaybackStop {
                    position_ticks: i.position_ticks,
                    played: i.played,
                    session_id: i.session_id,
                },
            ),
            Event::MarkPlayed(i) => {
                (i.user_id, i.media_id, MediaTrackerEvent::MarkPlayed)
            }
            Event::MarkUnplayed(i) => {
                (i.user_id, i.media_id, MediaTrackerEvent::MarkUnplayed)
            }
            Event::MarkFavorite(i) => {
                (i.user_id, i.media_id, MediaTrackerEvent::MarkFavorite)
            }
            Event::UnmarkFavorite(i) => {
                (i.user_id, i.media_id, MediaTrackerEvent::UnmarkFavorite)
            }
            Event::Rating(i) => (
                i.user_id,
                i.media_id,
                MediaTrackerEvent::Rating { rating: i.rating },
            ),
            _ => return Ok(()),
        };

        if !self
            .ctx
            .addons
            .has_media_tracker()
        {
            return Ok(());
        }

        let kind = tracker_event.kind();
        let wanted: Vec<db::UserMediaTracker> = db::UserMediaTracker::list_for_user(
            &self
                .ctx
                .db,
            user_id,
        )
        .await?
        .into_iter()
        .filter(|t| t.status == db::MediaTrackerStatus::Connected && t.wants(kind))
        .filter(|t| {
            self.ctx
                .addons
                .media_tracker_for(t.addon_id)
                .is_some_and(|a| {
                    a.capabilities()
                        .supports(kind)
                })
        })
        .collect();

        if wanted.is_empty() {
            return Ok(());
        }

        let Some(mut media) = db::Media::get_by_id(
            &self
                .ctx
                .db,
            &media_id,
        )
        .await?
        else {
            return Ok(());
        };

        let Some(target) = resolve_target(&self.ctx, &mut media).await? else {
            return Ok(());
        };

        for tracker in &wanted {
            let session_id = match &tracker_event {
                MediaTrackerEvent::PlaybackStart { session_id, .. }
                | MediaTrackerEvent::PlaybackProgress { session_id, .. }
                | MediaTrackerEvent::PlaybackStop { session_id, .. } => {
                    session_id.as_str()
                }
                _ => "",
            };
            let dedupe_key = match &tracker_event {
                MediaTrackerEvent::PlaybackStart { session_id, .. }
                | MediaTrackerEvent::PlaybackStop { session_id, .. }
                    if !session_id.is_empty() =>
                {
                    format!("{kind}:{session_id}")
                }
                MediaTrackerEvent::PlaybackProgress {
                    position_ticks,
                    is_paused,
                    session_id,
                } if !session_id.is_empty() => {
                    format!("{kind}:{session_id}:{position_ticks}:{is_paused}")
                }
                _ => crate::common::get_uuid().to_string(),
            };
            sqlx::query(
                "INSERT OR IGNORE INTO media_tracker_outbox \
                 (id, user_media_tracker_id, session_id, event_kind, event_json, target_json, \
                  dedupe_key, created_at, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
            )
            .bind(crate::common::get_uuid())
            .bind(tracker.id)
            .bind(session_id)
            .bind(kind.to_string())
            .bind(serde_json::to_string(&tracker_event)?)
            .bind(serde_json::to_string(&target)?)
            .bind(dedupe_key)
            .bind(Utc::now().naive_utc())
            .execute(&self.ctx.db)
            .await?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
struct MediaTrackerOutboxRow {
    id: Uuid,
    user_media_tracker_id: Uuid,
    event_json: String,
    target_json: String,
    attempts: i64,
}

fn retry_seconds(attempt: i64) -> i64 {
    (30_i64.saturating_mul(
        2_i64.saturating_pow(
            attempt
                .saturating_sub(1)
                .min(7) as u32,
        ),
    ))
    .min(3600)
}

async fn schedule_retry(
    ctx: &AppContext,
    row: &MediaTrackerOutboxRow,
    error: &crate::addons::media_tracker::MediaTrackerError,
) -> Result<()> {
    let attempt = row.attempts + 1;
    let provider_delay = error
        .retry_delay()
        .map(|delay| {
            delay
                .as_secs()
                .min(i64::MAX as u64) as i64
        })
        .unwrap_or_default();
    let delay = retry_seconds(attempt).max(provider_delay);
    let terminal = !error.is_retryable() || attempt >= 12;
    sqlx::query(
        "UPDATE media_tracker_outbox SET status = ?2, attempts = ?3, next_attempt_at = ?4, \
         last_error = ?5, updated_at = ?6 WHERE id = ?1",
    )
    .bind(row.id)
    .bind(if terminal { "failed" } else { "pending" })
    .bind(attempt)
    .bind(Utc::now().naive_utc() + chrono::Duration::seconds(delay))
    .bind(error.to_string())
    .bind(Utc::now().naive_utc())
    .execute(&ctx.db)
    .await?;
    Ok(())
}

async fn deliver_outbox_row(
    ctx: &AppContext,
    row: MediaTrackerOutboxRow,
) -> Result<()> {
    let Some(mut tracker) =
        db::UserMediaTracker::get(&ctx.db, row.user_media_tracker_id).await?
    else {
        sqlx::query("DELETE FROM media_tracker_outbox WHERE id = ?1")
            .bind(row.id)
            .execute(&ctx.db)
            .await?;
        return Ok(());
    };
    let Some(addon) = ctx
        .addons
        .media_tracker_for(tracker.addon_id)
    else {
        sqlx::query("DELETE FROM media_tracker_outbox WHERE id = ?1")
            .bind(row.id)
            .execute(&ctx.db)
            .await?;
        return Ok(());
    };
    let event: MediaTrackerEvent = serde_json::from_str(&row.event_json)?;
    let target: MediaTrackerTarget = serde_json::from_str(&row.target_json)?;
    let tctx = MediaTrackerCtx {
        config: Arc::new(
            ctx.config
                .clone(),
        ),
    };

    let mut result = addon
        .on_event(&event, &target, &tracker.credentials, &tctx)
        .await;
    if result
        .as_ref()
        .is_err_and(|error| error.requires_reauth())
    {
        match addon
            .refresh(&tracker.credentials, &tctx)
            .await
        {
            Ok(credentials) => {
                db::UserMediaTracker::replace_credentials(
                    &ctx.db,
                    tracker.id,
                    &credentials,
                )
                .await?;
                tracker.credentials = credentials;
                result = addon
                    .on_event(&event, &target, &tracker.credentials, &tctx)
                    .await;
            }
            Err(refresh_error) => result = Err(refresh_error),
        }
    }

    match result {
        Ok(()) => {
            sqlx::query(
                "UPDATE media_tracker_outbox SET status = 'delivered', updated_at = ?2 \
                 WHERE id = ?1",
            )
            .bind(row.id)
            .bind(Utc::now().naive_utc())
            .execute(&ctx.db)
            .await?;
            sqlx::query(
                "DELETE FROM media_tracker_outbox WHERE status = 'delivered' AND updated_at < ?1",
            )
            .bind(Utc::now().naive_utc() - chrono::Duration::days(7))
            .execute(&ctx.db)
            .await?;
            db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
        }
        Err(error) => {
            db::UserMediaTracker::mark_failure(&ctx.db, tracker.id, &error).await?;
            schedule_retry(ctx, &row, &error).await?;
        }
    }
    Ok(())
}

async fn next_outbox_row(ctx: &AppContext) -> Result<Option<MediaTrackerOutboxRow>> {
    Ok(sqlx::query_as::<_, MediaTrackerOutboxRow>(
        "SELECT o.id, o.user_media_tracker_id, o.event_json, o.target_json, o.attempts \
         FROM media_tracker_outbox o \
         WHERE o.status = 'pending' AND o.next_attempt_at <= ?1 \
         AND NOT EXISTS (SELECT 1 FROM media_tracker_outbox earlier \
             WHERE earlier.user_media_tracker_id = o.user_media_tracker_id \
             AND earlier.status = 'pending' \
             AND (earlier.created_at < o.created_at \
                  OR (earlier.created_at = o.created_at AND earlier.id < o.id))) \
         ORDER BY o.created_at ASC, o.id ASC LIMIT 1",
    )
    .bind(Utc::now().naive_utc())
    .fetch_optional(&ctx.db)
    .await?)
}

/// Starts the durable Trakt delivery loop. Rows remain pending across process
/// restarts and are retained briefly after delivery to deduplicate late client
/// stop reports for the same playback session.
pub fn spawn_outbox_worker(ctx: AppContext) {
    tokio::spawn(async move {
        info!("media tracker outbox worker started");
        loop {
            match next_outbox_row(&ctx).await {
                Ok(Some(row)) => {
                    if let Err(error) = deliver_outbox_row(&ctx, row).await {
                        warn!(error = %error, "media tracker outbox delivery failed");
                        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    }
                }
                Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(2)).await,
                Err(error) => {
                    warn!(error = %error, "could not read media tracker outbox");
                    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        addons::{
            Addon, AddonCapabilities, AddonKind, AddonPresetRef, AddonRuntime,
            media_tracker::{
                MediaTrackerAddon, MediaTrackerCapabilities, MediaTrackerCredentials,
                MediaTrackerEventKind, MediaTrackerResult,
            },
        },
        db::{MediaTrackerStatus, UserMediaTracker},
        integration_test::new_test_server,
        signals::MarkPlayedInfo,
    };
    use async_trait::async_trait;
    use chrono::Utc;
    use std::sync::Arc;

    struct StubMediaTracker(Vec<MediaTrackerEventKind>);

    impl StubMediaTracker {
        fn everything() -> Self {
            Self(vec![
                MediaTrackerEventKind::PlaybackStart,
                MediaTrackerEventKind::PlaybackProgress,
                MediaTrackerEventKind::PlaybackStop,
                MediaTrackerEventKind::MarkPlayed,
                MediaTrackerEventKind::MarkUnplayed,
                MediaTrackerEventKind::MarkFavorite,
                MediaTrackerEventKind::UnmarkFavorite,
                MediaTrackerEventKind::Rating,
            ])
        }
    }

    impl AddonKind for StubMediaTracker {
        fn id(&self) -> &'static str {
            "scripted"
        }
    }

    #[test]
    fn imported_progress_is_converted_to_database_seconds() {
        assert_eq!(
            progress_seconds(RemoteProgress::Ticks(125 * 10_000_000), None),
            Some(125)
        );
        assert_eq!(
            progress_seconds(RemoteProgress::Percent(25.0), Some(400)),
            Some(100)
        );
        assert_eq!(progress_seconds(RemoteProgress::Percent(25.0), None), None);
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
        let addon = crate::integration_test::register_media_tracker(
            ctx,
            name,
            Arc::new(StubMediaTracker::everything()),
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

    async fn user_id(ctx: &AppContext) -> Uuid {
        db::User::get_by_username(&ctx.db, "test")
            .await
            .unwrap()
            .unwrap()
            .id
    }

    /// The walk to the series follows `parent_id`, which an episode saved
    /// without a season row above it does not have.
    #[tokio::test]
    async fn a_flat_episode_still_reaches_its_series() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;

        let external_ids = db::ExternalIds {
            imdb: db::NonEmptyString::try_new("tt0306414".to_string()).ok(),
            tmdb: Some(1438),
            ..Default::default()
        };
        let mut series = db::Media {
            id: Uuid::from(&db::MediaIdRaw {
                kind: db::MediaKind::Series,
                external_ids: external_ids.clone(),
                season: None,
                episode: None,
            }),
            title: "The Wire".into(),
            kind: db::MediaKind::Series,
            external_ids,
            ..Default::default()
        };
        series
            .save(&ctx.db)
            .await
            .unwrap();

        let mut episode = db::Media {
            title: "The Target".into(),
            kind: db::MediaKind::Episode,
            grandparent_id: Some(series.id),
            idx: Some(1),
            parent_idx: Some(1),
            external_ids: db::ExternalIds {
                imdb: db::NonEmptyString::try_new("tt0749451".to_string()).ok(),
                tvdb: Some(299034),
                ..Default::default()
            },
            ..Default::default()
        };
        episode
            .save(&ctx.db)
            .await
            .unwrap();

        let target = resolve_target(ctx, &mut episode)
            .await
            .unwrap()
            .expect("an episode alone identifies nothing");

        assert_eq!(
            target
                .series
                .expect("no season row is not no series")
                .ids
                .tmdb,
            Some(1438)
        );
    }

    #[tokio::test]
    async fn subscriber_delivers_to_connected_trackers() {
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
        let uid = user_id(ctx).await;

        let sub = MediaTrackerSubscriber { ctx: ctx.clone() };
        let result = sub
            .handle(Event::MarkPlayed(MarkPlayedInfo {
                user_id: uid,
                media_id: media.id,
            }))
            .await;

        assert!(result.is_ok());
        let queued: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM media_tracker_outbox WHERE user_media_tracker_id = ?1",
        )
        .bind(tracker)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(queued, 1);
    }

    #[tokio::test]
    async fn subscriber_ignores_playback_progress_for_mark_played_tracker() {
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
        let uid = user_id(ctx).await;

        let sub = MediaTrackerSubscriber { ctx: ctx.clone() };
        // The tracker is connected but did not opt into progress events, so
        // invoking the subscriber directly still must enqueue nothing.
        let result = sub
            .handle(Event::PlaybackProgress(crate::signals::PlaybackContext {
                user_id: uid,
                media_id: media.id,
                position_ticks: 1000,
                is_paused: false,
                ..Default::default()
            }))
            .await;

        assert!(result.is_ok());
        let queued: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM media_tracker_outbox WHERE user_media_tracker_id = ?1",
        )
        .bind(tracker)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(queued, 0);
    }
}
