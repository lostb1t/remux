//! Describing the item a delivery names, and the subscriber that scrobbles it.

use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Instant,
};

use anyhow::{Error, Result};
use async_trait::async_trait;
use chrono::{Datelike, Utc};
use futures::FutureExt;
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

const IMPORT_PROGRESS_LOG_INTERVAL: usize = 250;

fn should_log_import_progress(processed: usize, total: usize) -> bool {
    processed == total || processed % IMPORT_PROGRESS_LOG_INTERVAL == 0
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

fn watch_root(watch: &RemoteWatch) -> Option<(String, db::Media)> {
    let kind = if watch
        .season
        .is_some()
        && watch
            .episode
            .is_some()
    {
        db::MediaKind::Series
    } else {
        db::MediaKind::Movie
    };
    let raw = db::MediaIdRaw {
        kind: kind.clone(),
        external_ids: watch
            .ids
            .clone(),
        season: None,
        episode: None,
    };
    let identity = raw.identity_key()?;
    let released_at = watch
        .year
        .and_then(|year| chrono::NaiveDate::from_ymd_opt(year, 1, 1))
        .and_then(|date| date.and_hms_opt(0, 0, 0));
    Some((
        identity,
        db::Media {
            id: Uuid::from(&raw),
            title: watch
                .title
                .clone(),
            kind,
            released_at,
            external_ids: watch
                .ids
                .clone(),
            ..Default::default()
        },
    ))
}

async fn tag_history_catalog_roots(
    ctx: &AppContext,
    media_ids: &HashSet<Uuid>,
    tag: &str,
) -> Result<()> {
    if media_ids.is_empty() {
        return Ok(());
    }
    const CHUNK_SIZE: usize = 400;
    for ids in media_ids
        .iter()
        .copied()
        .collect::<Vec<_>>()
        .chunks(CHUNK_SIZE)
    {
        let mut qb = sqlx::QueryBuilder::new(
            "INSERT OR IGNORE INTO media_tags (media_id, tag) ",
        );
        qb.push_values(ids, |mut row, id| {
            row.push_bind(*id)
                .push_bind(tag);
        });
        qb.build()
            .execute(&ctx.db)
            .await?;
    }
    Ok(())
}

/// Materialize top-level movies and shows referenced by a remote history
/// snapshot. Existing roots are only refreshed when at least one of their
/// events could not be matched (normally a missing episode tree).
async fn materialize_history_catalog(
    ctx: &AppContext,
    tracker_id: Uuid,
    grouped: &HashMap<String, (db::MediaIdRaw, RemoteWatch)>,
    existing_roots: &RootMediaLookup,
    unmatched_root_keys: &HashSet<String>,
    tag: &str,
) -> Result<()> {
    let started = Instant::now();
    let mut roots = HashMap::<String, db::Media>::new();
    for (_, watch) in grouped.values() {
        if let Some((identity, media)) = watch_root(watch) {
            roots
                .entry(identity)
                .or_insert(media);
        }
    }

    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        roots = roots.len(),
        unmatched_roots = unmatched_root_keys.len(),
        "media tracker import catalog discovery complete"
    );

    let mut refresh = Vec::new();
    let mut root_ids = HashSet::new();
    let roots_total = roots.len();
    for (index, (identity, stub)) in roots
        .iter_mut()
        .enumerate()
    {
        let existing = existing_roots
            .find(&stub.kind, &stub.external_ids)
            .cloned();
        let missing = existing.is_none();
        if let Some(existing) = existing {
            let imported_ids = stub
                .external_ids
                .clone();
            *stub = existing;
            stub.external_ids
                .merge(&imported_ids, false);
        }
        if missing || unmatched_root_keys.contains(identity) {
            refresh.push(stub.clone());
        } else {
            root_ids.insert(stub.id);
        }
        let processed = index + 1;
        if should_log_import_progress(processed, roots_total) {
            info!(
                target: "remux_server::media_tracker_import",
                %tracker_id,
                processed,
                total = roots_total,
                progress_percent = processed.saturating_mul(100) / roots_total.max(1),
                refresh = refresh.len(),
                existing = root_ids.len(),
                elapsed_seconds = started.elapsed().as_secs_f64(),
                "media tracker import catalog matching progress"
            );
        }
    }

    if !refresh.is_empty() {
        let refresh_count = refresh.len();
        info!(
            target: "remux_server::media_tracker_import",
            %tracker_id,
            refresh = refresh_count,
            "media tracker import catalog metadata refresh started"
        );
        let original_ids: Vec<Uuid> = refresh
            .iter()
            .map(|media| media.id)
            .collect();
        let remapped = ctx
            .addons
            // Existing roots only need their tree walked to discover missing
            // episodes. A forced refresh also re-fetches metadata for every
            // already-known season and episode, which makes repeat history
            // imports unnecessarily expensive.
            .process_meta_batch(refresh, ctx, false, None)
            .await?;
        root_ids.extend(
            original_ids
                .into_iter()
                .map(|id| {
                    remapped
                        .get(&id)
                        .copied()
                        .unwrap_or(id)
                }),
        );
        info!(
            target: "remux_server::media_tracker_import",
            %tracker_id,
            refreshed = refresh_count,
            elapsed_seconds = started.elapsed().as_secs_f64(),
            "media tracker import catalog metadata refresh complete"
        );
    }

    tag_history_catalog_roots(ctx, &root_ids, tag).await?;
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        tagged_roots = root_ids.len(),
        elapsed_seconds = started.elapsed().as_secs_f64(),
        "media tracker import catalog materialization complete"
    );
    Ok(())
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

#[derive(Default)]
struct RootMediaLookup {
    ids: HashMap<String, Uuid>,
    media: HashMap<Uuid, db::Media>,
}

fn root_lookup_keys(kind: &db::MediaKind, ids: &db::ExternalIds) -> Vec<String> {
    let prefix = match kind {
        db::MediaKind::Movie => "movie",
        db::MediaKind::Series => "series",
        _ => return Vec::new(),
    };
    let mut keys = Vec::with_capacity(5);
    if let Some(imdb) = ids
        .imdb
        .as_deref()
    {
        keys.push(format!("{prefix}:imdb:{imdb}"));
    }
    if let Some(tmdb) = ids.tmdb {
        keys.push(format!("{prefix}:tmdb:{tmdb}"));
    }
    if let Some(tvdb) = ids.tvdb {
        keys.push(format!("{prefix}:tvdb:{tvdb}"));
    }
    if let Some(custom) = ids
        .custom_stremio_id
        .as_deref()
    {
        keys.push(format!("{prefix}:custom:{custom}"));
    }
    if let Some(kitsu) = ids.kitsu {
        keys.push(format!("{prefix}:kitsu:{kitsu}"));
    }
    keys
}

impl RootMediaLookup {
    fn insert(&mut self, media: db::Media) {
        for key in root_lookup_keys(&media.kind, &media.external_ids) {
            self.ids
                .entry(key)
                .or_insert(media.id);
        }
        self.media
            .entry(media.id)
            .or_insert(media);
    }

    fn find(&self, kind: &db::MediaKind, ids: &db::ExternalIds) -> Option<&db::Media> {
        root_lookup_keys(kind, ids)
            .into_iter()
            .find_map(|key| {
                self.ids
                    .get(&key)
            })
            .and_then(|id| {
                self.media
                    .get(id)
            })
    }
}

async fn load_root_media(
    ctx: &AppContext,
    grouped: &HashMap<String, (db::MediaIdRaw, RemoteWatch)>,
) -> Result<RootMediaLookup> {
    let mut imdb = HashSet::new();
    let mut tmdb = HashSet::new();
    let mut tvdb = HashSet::new();
    let mut custom = HashSet::new();
    let mut kitsu = HashSet::new();
    for (_, watch) in grouped.values() {
        if let Some(value) = watch
            .ids
            .imdb
            .as_deref()
        {
            imdb.insert(value.to_owned());
        }
        if let Some(value) = watch
            .ids
            .tmdb
        {
            tmdb.insert(value);
        }
        if let Some(value) = watch
            .ids
            .tvdb
        {
            tvdb.insert(value);
        }
        if let Some(value) = watch
            .ids
            .custom_stremio_id
            .as_deref()
        {
            custom.insert(value.to_owned());
        }
        if let Some(value) = watch
            .ids
            .kitsu
        {
            kitsu.insert(value);
        }
    }

    let mut found = HashMap::<Uuid, db::Media>::new();
    let imdb = imdb
        .into_iter()
        .collect::<Vec<_>>();
    for values in imdb.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.imdb') IN (",
        );
        let mut separated = query.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
        separated.push_unseparated(")");
        for media in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            found.insert(media.id, media);
        }
    }
    let tmdb = tmdb
        .into_iter()
        .collect::<Vec<_>>();
    for values in tmdb.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.tmdb') IN (",
        );
        let mut separated = query.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
        separated.push_unseparated(")");
        for media in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            found.insert(media.id, media);
        }
    }
    let tvdb = tvdb
        .into_iter()
        .collect::<Vec<_>>();
    for values in tvdb.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.tvdb') IN (",
        );
        let mut separated = query.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
        separated.push_unseparated(")");
        for media in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            found.insert(media.id, media);
        }
    }
    let custom = custom
        .into_iter()
        .collect::<Vec<_>>();
    for values in custom.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.custom_stremio_id') IN (",
        );
        let mut separated = query.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
        separated.push_unseparated(")");
        for media in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            found.insert(media.id, media);
        }
    }
    let kitsu = kitsu
        .into_iter()
        .collect::<Vec<_>>();
    for values in kitsu.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind IN ('movie', 'series') \
             AND json_extract(external_ids, '$.kitsu') IN (",
        );
        let mut separated = query.separated(", ");
        for value in values {
            separated.push_bind(value);
        }
        separated.push_unseparated(")");
        for media in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            found.insert(media.id, media);
        }
    }

    let mut lookup = RootMediaLookup::default();
    for media in found.into_values() {
        lookup.insert(media);
    }
    Ok(lookup)
}

async fn bulk_find_media_for_watches(
    ctx: &AppContext,
    grouped: &HashMap<String, (db::MediaIdRaw, RemoteWatch)>,
) -> Result<(HashMap<String, db::Media>, RootMediaLookup)> {
    let roots = load_root_media(ctx, grouped).await?;
    let series_ids = grouped
        .values()
        .filter_map(|(_, watch)| {
            watch.season?;
            watch.episode?;
            roots
                .find(&db::MediaKind::Series, &watch.ids)
                .map(|series| series.id)
        })
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();

    let mut episodes = HashMap::<(Uuid, i64, i64), db::Media>::new();
    for ids in series_ids.chunks(db::CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "SELECT * FROM media WHERE kind = 'episode' AND grandparent_id IN (",
        );
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");
        for episode in query
            .build_query_as::<db::Media>()
            .fetch_all(&ctx.db)
            .await?
        {
            if let (Some(series_id), Some(season), Some(number)) =
                (episode.grandparent_id, episode.parent_idx, episode.idx)
            {
                episodes.insert((series_id, season, number), episode);
            }
        }
    }

    let mut matched = HashMap::with_capacity(grouped.len());
    for (identity, (_, watch)) in grouped {
        let media = match (watch.season, watch.episode) {
            (Some(season), Some(episode)) => roots
                .find(&db::MediaKind::Series, &watch.ids)
                .and_then(|series| episodes.get(&(series.id, season, episode))),
            _ => roots.find(&db::MediaKind::Movie, &watch.ids),
        };
        if let Some(media) = media {
            matched.insert(identity.clone(), media.clone());
        }
    }
    Ok((matched, roots))
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

#[derive(Debug)]
struct PendingImportState {
    state: db::UserMediaState,
    dirty: bool,
}

fn merge_import_state_rows(
    destination: db::UserMediaState,
    source: db::UserMediaState,
    media_id: Uuid,
) -> db::UserMediaState {
    let (primary, other) = if destination.last_played_at >= source.last_played_at {
        (&destination, &source)
    } else {
        (&source, &destination)
    };
    db::UserMediaState {
        user_id: destination.user_id,
        media_id,
        media_raw: primary
            .media_raw
            .clone()
            .or_else(|| {
                other
                    .media_raw
                    .clone()
            }),
        stream_id: primary
            .stream_id
            .or(other.stream_id),
        favorite: destination.favorite || source.favorite,
        play_count: destination
            .play_count
            .max(source.play_count),
        played_at: destination
            .played_at
            .max(source.played_at),
        playback_position: primary.playback_position,
        last_played_at: destination
            .last_played_at
            .max(source.last_played_at),
        subtitle_idx: primary.subtitle_idx,
        audio_idx: primary.audio_idx,
        rating: primary
            .rating
            .or(other.rating),
    }
}

fn take_import_state(
    user_id: Uuid,
    media_id: Uuid,
    identity: &str,
    stored: &mut HashMap<Uuid, db::UserMediaState>,
    pending: &mut HashMap<Uuid, PendingImportState>,
    raw_to_media: &HashMap<String, Uuid>,
    delete_ids: &mut HashSet<Uuid>,
) -> PendingImportState {
    let mut selected = pending
        .remove(&media_id)
        .or_else(|| {
            stored
                .remove(&media_id)
                .map(|state| PendingImportState {
                    state,
                    dirty: false,
                })
        });

    if let Some(source_id) = raw_to_media
        .get(identity)
        .copied()
        .filter(|source_id| *source_id != media_id)
    {
        let source = pending
            .remove(&source_id)
            .or_else(|| {
                stored
                    .remove(&source_id)
                    .map(|state| PendingImportState {
                        state,
                        dirty: false,
                    })
            });
        if let Some(source) = source {
            delete_ids.insert(source_id);
            selected = Some(match selected {
                Some(destination) => PendingImportState {
                    state: merge_import_state_rows(
                        destination.state,
                        source.state,
                        media_id,
                    ),
                    dirty: true,
                },
                None => PendingImportState {
                    state: db::UserMediaState {
                        media_id,
                        ..source.state
                    },
                    dirty: true,
                },
            });
        }
    }

    let mut selected = selected.unwrap_or_else(|| PendingImportState {
        state: db::UserMediaState {
            user_id,
            media_id,
            media_raw: Some(identity.to_owned()),
            ..Default::default()
        },
        dirty: true,
    });
    selected
        .state
        .user_id = user_id;
    selected
        .state
        .media_id = media_id;
    if selected
        .state
        .media_raw
        .is_none()
    {
        selected
            .state
            .media_raw = Some(identity.to_owned());
        selected.dirty = true;
    }
    selected
}

async fn persist_import_states(
    ctx: &AppContext,
    user_id: Uuid,
    delete_ids: &HashSet<Uuid>,
    states: &[db::UserMediaState],
) -> Result<()> {
    if delete_ids.is_empty() && states.is_empty() {
        return Ok(());
    }
    let mut tx = ctx
        .db
        .begin()
        .await?;
    let delete_ids = delete_ids
        .iter()
        .copied()
        .collect::<Vec<_>>();
    for ids in delete_ids.chunks(db::CHUNK_SIZE) {
        let mut query =
            sqlx::QueryBuilder::new("DELETE FROM user_media_state WHERE user_id = ");
        query
            .push_bind(user_id)
            .push(" AND media_id IN (");
        let mut separated = query.separated(", ");
        for id in ids {
            separated.push_bind(id);
        }
        separated.push_unseparated(")");
        query
            .build()
            .execute(&mut *tx)
            .await?;
    }

    // Twelve binds per row. Keep each statement below SQLite's conservative
    // 999-variable limit even when Remux is built against an older SQLite.
    const STATE_CHUNK_SIZE: usize = 75;
    for states in states.chunks(STATE_CHUNK_SIZE) {
        let mut query = sqlx::QueryBuilder::new(
            "INSERT INTO user_media_state (user_id, media_id, media_raw, stream_id, favorite, \
             play_count, played_at, playback_position, last_played_at, subtitle_idx, audio_idx, rating) ",
        );
        query.push_values(states, |mut row, state| {
            row.push_bind(state.user_id)
                .push_bind(state.media_id)
                .push_bind(&state.media_raw)
                .push_bind(state.stream_id)
                .push_bind(state.favorite)
                .push_bind(state.play_count)
                .push_bind(state.played_at)
                .push_bind(state.playback_position)
                .push_bind(state.last_played_at)
                .push_bind(state.subtitle_idx)
                .push_bind(state.audio_idx)
                .push_bind(state.rating);
        });
        query.push(
            " ON CONFLICT(user_id, media_id) DO UPDATE SET \
             media_raw = excluded.media_raw, stream_id = excluded.stream_id, \
             favorite = excluded.favorite, play_count = excluded.play_count, \
             played_at = excluded.played_at, playback_position = excluded.playback_position, \
             last_played_at = excluded.last_played_at, subtitle_idx = excluded.subtitle_idx, \
             audio_idx = excluded.audio_idx, rating = excluded.rating",
        );
        query
            .build()
            .execute(&mut *tx)
            .await?;
    }
    tx.commit()
        .await?;
    Ok(())
}

/// Pull a complete provider snapshot and merge it into Remux without emitting
/// user-data signals, so an import can never echo back to the provider.
pub async fn import_tracker_history(
    ctx: &AppContext,
    tracker_id: Uuid,
) -> Result<ImportStats> {
    let started = Instant::now();
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
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        user_id = %tracker.user_id,
        addon_id = %tracker.addon_id,
        "media tracker import history fetch started"
    );
    let mut watches_result = addon
        .import_history(&tracker.credentials, &tctx)
        .await;
    if watches_result
        .as_ref()
        .is_err_and(|error| error.requires_reauth())
    {
        info!(
            target: "remux_server::media_tracker_import",
            %tracker_id,
            "media tracker import refreshing provider credentials"
        );
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
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        fetched = stats.fetched,
        elapsed_seconds = started.elapsed().as_secs_f64(),
        "media tracker import history fetch complete"
    );
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
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        fetched = stats.fetched,
        unique = grouped.len(),
        invalid = stats.skipped,
        duplicates = stats
            .fetched
            .saturating_sub(stats.skipped)
            .saturating_sub(grouped.len() as i64),
        "media tracker import history grouping complete"
    );

    // Resolve the entire snapshot in batches before materialization so only
    // shows with missing episode trees are refreshed.
    let (initial_matches, existing_roots) =
        bulk_find_media_for_watches(ctx, &grouped).await?;
    let mut unmatched_root_keys = HashSet::new();
    for (identity, (_, watch)) in &grouped {
        if !initial_matches.contains_key(identity)
            && let Some((root_identity, _)) = watch_root(watch)
        {
            unmatched_root_keys.insert(root_identity);
        }
    }
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        total = grouped.len(),
        matched = initial_matches.len(),
        unmatched_roots = unmatched_root_keys.len(),
        elapsed_seconds = started.elapsed().as_secs_f64(),
        "media tracker import initial batch matching complete"
    );

    let mut matched_media = initial_matches;
    if let Some(tag) = addon.history_catalog_tag() {
        materialize_history_catalog(
            ctx,
            tracker_id,
            &grouped,
            &existing_roots,
            &unmatched_root_keys,
            tag,
        )
        .await?;
        // Catalog hydration has completed. Re-run the batched resolver so
        // newly-created movies and episodes receive watch state below.
        matched_media = bulk_find_media_for_watches(ctx, &grouped)
            .await?
            .0;
        info!(
            target: "remux_server::media_tracker_import",
            %tracker_id,
            total = grouped.len(),
            matched = matched_media.len(),
            elapsed_seconds = started.elapsed().as_secs_f64(),
            "media tracker import post-catalog batch matching complete"
        );
    }

    let user = db::User::get_by_id(&ctx.db, &tracker.user_id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("tracker user not found"))?;
    let stored_states = sqlx::query_as::<_, db::UserMediaState>(
        "SELECT * FROM user_media_state WHERE user_id = ?1",
    )
    .bind(user.id)
    .fetch_all(&ctx.db)
    .await?;
    let raw_to_media = stored_states
        .iter()
        .filter_map(|state| {
            state
                .media_raw
                .clone()
                .map(|raw| (raw, state.media_id))
        })
        .collect::<HashMap<_, _>>();
    let mut stored_states = stored_states
        .into_iter()
        .map(|state| (state.media_id, state))
        .collect::<HashMap<_, _>>();
    let mut pending_states = HashMap::<Uuid, PendingImportState>::new();
    let mut delete_state_ids = HashSet::new();
    let apply_total = grouped.len();
    for (index, (identity, (raw, watch))) in grouped
        .into_iter()
        .enumerate()
    {
        let media = matched_media.remove(&identity);
        let (media_id, runtime) = if let Some(media) = media {
            stats.matched += 1;
            (media.id, media.runtime)
        } else {
            stats.deferred += 1;
            (Uuid::from(&raw), None)
        };
        let mut pending = take_import_state(
            user.id,
            media_id,
            &identity,
            &mut stored_states,
            &mut pending_states,
            &raw_to_media,
            &mut delete_state_ids,
        );

        let before = (
            pending
                .state
                .play_count,
            pending
                .state
                .played_at,
            pending
                .state
                .last_played_at,
            pending
                .state
                .playback_position,
        );
        let incoming_count = watch
            .play_count
            .unwrap_or(if watch.watched { 1 } else { 0 });
        if incoming_count
            > pending
                .state
                .play_count
        {
            pending
                .state
                .play_count = incoming_count;
        }
        if watch.watched
            && pending
                .state
                .play_count
                == 0
        {
            pending
                .state
                .play_count = 1;
        }
        if let Some(watched_at) = watch.watched_at {
            pending
                .state
                .played_at = pending
                .state
                .played_at
                .max(Some(watched_at));
            pending
                .state
                .last_played_at = pending
                .state
                .last_played_at
                .max(Some(watched_at));
        }
        if watch.watched && watch.progress_at <= watch.watched_at {
            pending
                .state
                .playback_position = 0;
        }
        if let (Some(progress), Some(progress_at)) = (watch.progress, watch.progress_at)
        {
            if pending
                .state
                .last_played_at
                .is_none_or(|local| local <= progress_at)
            {
                if let Some(seconds) = progress_seconds(progress, runtime) {
                    pending
                        .state
                        .playback_position = seconds;
                    pending
                        .state
                        .last_played_at = Some(progress_at);
                }
            }
        }
        let after = (
            pending
                .state
                .play_count,
            pending
                .state
                .played_at,
            pending
                .state
                .last_played_at,
            pending
                .state
                .playback_position,
        );
        if after != before {
            pending.dirty = true;
            stats.updated += 1;
        }
        pending_states.insert(media_id, pending);
        let processed = index + 1;
        if should_log_import_progress(processed, apply_total) {
            info!(
                target: "remux_server::media_tracker_import",
                %tracker_id,
                processed,
                total = apply_total,
                progress_percent = processed.saturating_mul(100) / apply_total.max(1),
                matched = stats.matched,
                updated = stats.updated,
                deferred = stats.deferred,
                skipped = stats.skipped,
                elapsed_seconds = started.elapsed().as_secs_f64(),
                "media tracker import watch-state progress"
            );
        }
    }
    let states_to_write = pending_states
        .into_values()
        .filter_map(|pending| {
            pending
                .dirty
                .then_some(pending.state)
        })
        .collect::<Vec<_>>();
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        writes = states_to_write.len(),
        remapped = delete_state_ids.len(),
        "media tracker import watch-state batch write started"
    );
    persist_import_states(ctx, user.id, &delete_state_ids, &states_to_write).await?;
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        writes = states_to_write.len(),
        remapped = delete_state_ids.len(),
        elapsed_seconds = started.elapsed().as_secs_f64(),
        "media tracker import watch-state batch write complete"
    );
    db::UserMediaTracker::mark_success(&ctx.db, tracker.id).await?;
    info!(
        target: "remux_server::media_tracker_import",
        %tracker_id,
        fetched = stats.fetched,
        matched = stats.matched,
        updated = stats.updated,
        deferred = stats.deferred,
        skipped = stats.skipped,
        elapsed_seconds = started.elapsed().as_secs_f64(),
        "media tracker import complete"
    );
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
                    let delivery = std::panic::AssertUnwindSafe(deliver_outbox_row(
                        &ctx,
                        row.clone(),
                    ))
                    .catch_unwind()
                    .await;
                    match delivery {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => {
                            warn!(error = %error, "media tracker outbox delivery failed");
                            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                        }
                        Err(_) => {
                            // Provider integrations must not be able to kill the durable
                            // delivery loop. Quarantine this row so later playback events
                            // for the same tracker can continue to drain.
                            let error = crate::addons::media_tracker::MediaTrackerError::permanent(
                                "media tracker provider panicked while delivering this event",
                            );
                            warn!(
                                outbox_id = %row.id,
                                tracker_id = %row.user_media_tracker_id,
                                "media tracker provider panicked; quarantining outbox row"
                            );
                            if let Err(mark_error) = db::UserMediaTracker::mark_failure(
                                &ctx.db,
                                row.user_media_tracker_id,
                                &error,
                            )
                            .await
                            {
                                warn!(error = %mark_error, "could not record media tracker panic");
                            }
                            if let Err(schedule_error) =
                                schedule_retry(&ctx, &row, &error).await
                            {
                                warn!(
                                    error = %schedule_error,
                                    "could not quarantine panicked media tracker outbox row"
                                );
                                tokio::time::sleep(std::time::Duration::from_secs(5))
                                    .await;
                            }
                        }
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

    fn remote_watch(season: Option<i64>, episode: Option<i64>) -> RemoteWatch {
        RemoteWatch {
            title: "The Wire".into(),
            year: Some(2002),
            ids: db::ExternalIds {
                tmdb: Some(1438),
                tvdb: Some(79126),
                ..Default::default()
            },
            season,
            episode,
            watched: true,
            play_count: Some(1),
            progress: None,
            watched_at: None,
            progress_at: None,
            favorite: None,
            rating: None,
        }
    }

    #[test]
    fn episode_history_materializes_a_series_root() {
        let (_, root) = watch_root(&remote_watch(Some(1), Some(1))).unwrap();
        assert_eq!(root.kind, db::MediaKind::Series);
        assert_eq!(root.title, "The Wire");
        assert_eq!(
            root.external_ids
                .tmdb,
            Some(1438)
        );
        assert_eq!(
            root.released_at
                .unwrap()
                .date(),
            chrono::NaiveDate::from_ymd_opt(2002, 1, 1).unwrap()
        );
    }

    #[test]
    fn movie_history_materializes_a_movie_root() {
        let (_, root) = watch_root(&remote_watch(None, None)).unwrap();
        assert_eq!(root.kind, db::MediaKind::Movie);
    }

    #[test]
    fn root_matching_accepts_simkl_and_kitsu_fallback_ids() {
        let keys = root_lookup_keys(
            &db::MediaKind::Series,
            &db::ExternalIds {
                kitsu: Some(7442),
                custom_stremio_id: Some("simkl:39687".into()),
                ..Default::default()
            },
        );
        assert!(keys.contains(&"series:custom:simkl:39687".to_string()));
        assert!(keys.contains(&"series:kitsu:7442".to_string()));
    }

    #[tokio::test]
    async fn batch_history_matching_finds_movies_and_episodes() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let episode_index_exists: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type = 'index' \
             AND name = 'idx_media_trakt_episode_lookup'",
        )
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(episode_index_exists, 1);
        let movie = crate::integration_test::seed_movie(ctx).await;
        let episode = crate::integration_test::seed_episode(ctx).await;
        let movie_watch = RemoteWatch {
            title: movie
                .title
                .clone(),
            year: None,
            ids: movie
                .external_ids
                .clone(),
            season: None,
            episode: None,
            watched: true,
            play_count: Some(1),
            progress: None,
            watched_at: None,
            progress_at: None,
            favorite: None,
            rating: None,
        };
        let episode_watch = remote_watch(Some(1), Some(1));
        let mut grouped = HashMap::new();
        for watch in [movie_watch, episode_watch] {
            let (identity, raw) = watch_identity(&watch).unwrap();
            grouped.insert(identity, (raw, watch));
        }

        let (matched, _) = bulk_find_media_for_watches(ctx, &grouped)
            .await
            .unwrap();

        assert_eq!(matched.len(), 2);
        assert!(
            matched
                .values()
                .any(|media| media.id == movie.id)
        );
        assert!(
            matched
                .values()
                .any(|media| media.id == episode.id)
        );
    }

    #[tokio::test]
    async fn batch_state_write_remaps_an_identity_row_without_losing_data() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let uid = user_id(ctx).await;
        let old_id = Uuid::new_v4();
        let new_id = Uuid::new_v4();
        let identity = "episode:tvdb:79126:1:1";
        let old = db::UserMediaState {
            user_id: uid,
            media_id: old_id,
            media_raw: Some(identity.into()),
            favorite: true,
            play_count: 3,
            ..Default::default()
        };
        old.save(&ctx.db)
            .await
            .unwrap();

        let mut stored = HashMap::from([(old_id, old)]);
        let raw_to_media = HashMap::from([(identity.to_string(), old_id)]);
        let mut pending = HashMap::new();
        let mut deletes = HashSet::new();
        let mut state = take_import_state(
            uid,
            new_id,
            identity,
            &mut stored,
            &mut pending,
            &raw_to_media,
            &mut deletes,
        );
        state
            .state
            .play_count = 4;
        state.dirty = true;
        persist_import_states(ctx, uid, &deletes, &[state.state])
            .await
            .unwrap();

        let old_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(uid)
        .bind(old_id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        let remapped = sqlx::query_as::<_, db::UserMediaState>(
            "SELECT * FROM user_media_state WHERE user_id = ?1 AND media_id = ?2",
        )
        .bind(uid)
        .bind(new_id)
        .fetch_one(&ctx.db)
        .await
        .unwrap();
        assert_eq!(old_count, 0);
        assert_eq!(remapped.play_count, 4);
        assert!(remapped.favorite);
        assert_eq!(
            remapped
                .media_raw
                .as_deref(),
            Some(identity)
        );
    }

    #[tokio::test]
    async fn trakt_history_catalog_is_seeded_by_migration() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let catalog_id =
            Uuid::parse_str("7472616b-742d-4c69-6272-617279000001").unwrap();
        let catalog = db::Media::get_by_id(&ctx.db, &catalog_id)
            .await
            .unwrap()
            .expect("Trakt History catalog");
        assert_eq!(catalog.title, "Trakt History");
        assert_eq!(catalog.collection_kind, Some(db::CollectionKind::Smart));
        assert_eq!(
            catalog.collection_media_kind,
            Some(db::CollectionMediaKind::Mixed)
        );
        let filter = catalog
            .parse_smart_filter()
            .expect("smart filter");
        assert!(
            filter
                .groups
                .iter()
                .flat_map(|group| &group.rules)
                .any(|rule| matches!(
                    rule,
                    remux_sdks::remux::FilterRule::Tracked { value: true }
                ))
        );

        let movie = crate::integration_test::seed_movie(ctx).await;
        sqlx::query(
            "INSERT INTO media_tags (media_id, tag) VALUES (?1, 'source:Trakt')",
        )
        .bind(movie.id)
        .execute(&ctx.db)
        .await
        .unwrap();
        let user = db::User::get_by_id(&ctx.db, &user_id(ctx).await)
            .await
            .unwrap()
            .unwrap();
        let mut state = db::UserMediaState::get_or_new(&ctx.db, &user, &movie)
            .await
            .unwrap();
        state.playback_position = 60;
        state
            .save(&ctx.db)
            .await
            .unwrap();

        let contents = db::Media::get_by_filter(
            &ctx.db,
            &db::MediaFilter {
                kind: Some(vec![db::MediaKind::Movie, db::MediaKind::Series]),
                user_id: Some(user.id),
                filter_rules: Some(filter.clone()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(
            contents
                .records
                .iter()
                .map(|media| media.id)
                .collect::<Vec<_>>(),
            vec![movie.id],
            "resume-only Trakt state should appear in the catalog"
        );
    }

    #[tokio::test]
    async fn simkl_catalog_provider_is_seeded_by_migration() {
        let (_s, guard) = new_test_server()
            .await
            .unwrap();
        let ctx = &guard.0;
        let addon_id = Uuid::parse_str("73696d6b-6c00-0000-0000-000000000001").unwrap();
        let addon = Addon::get(&ctx.db, addon_id)
            .await
            .unwrap()
            .expect("Simkl provider");
        assert_eq!(addon.name, "Simkl");
        assert_eq!(
            addon
                .preset
                .kind,
            "simkl"
        );
        assert!(
            addon
                .resources
                .contains(&remux_sdks::stremio::ResourceType::Catalog)
        );
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
