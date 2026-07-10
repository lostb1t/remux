use anyhow::{Context, Result};
use async_trait::async_trait;
use chrono::Utc;
use sqlx::Row;
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, db};

pub struct BackfillWatchHistoryTask;

#[async_trait]
impl Task for BackfillWatchHistoryTask {
    fn key(&self) -> &str {
        "BackfillWatchHistory"
    }

    fn name(&self) -> &str {
        "Backfill Watch History"
    }

    fn description(&self) -> &str {
        "Rebuilds watch_history rows from existing user_media_state for recommendation inputs."
    }

    fn short_description(&self) -> &str {
        "Backfill watch_history from user playback state"
    }

    fn category(&self) -> TaskCategory {
        TaskCategory::Library
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        const CHUNK_SIZE: u32 = 500;

        let total_records: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) \
             FROM user_media_state \
             WHERE play_count > 0 OR (play_count = 0 AND playback_position > 0)",
        )
        .fetch_one(&ctx.db)
        .await?;
        let total = total_records.max(1) as usize;

        let mut processed = 0usize;
        let mut offset = 0u32;
        let fallback_time = Utc::now().naive_utc();

        loop {
            let states = db::UserMediaState::get_by_filter(
                &ctx.db,
                &db::UserMediaStateFilter {
                    played: Some(true),
                    resumable: Some(true),
                    limit: Some(CHUNK_SIZE),
                    offset: Some(offset),
                    ..Default::default()
                },
            )
            .await?
            .records;

            if states.is_empty() {
                break;
            }

            let media_ids: Vec<_> = states
                .iter()
                .map(|s| s.media_id)
                .collect();
            let media = db::Media::get_by_filter(
                &ctx.db,
                &db::MediaFilter {
                    id: Some(media_ids),
                    total_count: false,
                    ..Default::default()
                },
            )
            .await?
            .records;

            let media_runtimes: HashMap<_, _> = media
                .into_iter()
                .map(|m| {
                    (
                        m.id,
                        m.runtime
                            .unwrap_or(0),
                    )
                })
                .collect();

            let mut state_by_user: HashMap<Uuid, Vec<db::UserMediaState>> =
                HashMap::new();
            for state in states {
                state_by_user
                    .entry(state.user_id)
                    .or_default()
                    .push(state);
            }
            let scanned = state_by_user
                .values()
                .map(|values| values.len())
                .sum::<usize>();
            processed += scanned;

            for (user_id, user_states) in state_by_user {
                let mut user_media_ids = Vec::with_capacity(user_states.len());
                for state in &user_states {
                    user_media_ids.push(state.media_id);
                }

                let mut existing: HashSet<Uuid> = HashSet::new();
                if !user_media_ids.is_empty() {
                    let mut qb = sqlx::QueryBuilder::new(
                        "SELECT DISTINCT media_id FROM watch_history WHERE user_id = ",
                    );
                    qb.push_bind(user_id);
                    qb.push(" AND event_type = 'playback_stop' AND media_id IN (");
                    let mut sep = qb.separated(", ");
                    for media_id in &user_media_ids {
                        sep.push_bind(media_id);
                    }
                    qb.push(")");
                    if let Ok(rows) = qb
                        .build()
                        .fetch_all(&ctx.db)
                        .await
                    {
                        for row in rows {
                            let media_id: Uuid = row.get(0);
                            existing.insert(media_id);
                        }
                    }
                }

                for state in user_states {
                    if !existing.insert(state.media_id) {
                        continue;
                    }
                    let runtime = media_runtimes
                        .get(&state.media_id)
                        .copied()
                        .unwrap_or(0);
                    let position_ticks = state
                        .playback_position
                        .max(0)
                        * 10_000_000;
                    let completed = if runtime > 0 {
                        state.playback_position >= (runtime * 90 / 100)
                    } else {
                        state.play_count > 0
                    };
                    let created_at = state
                        .last_played_at
                        .or(state.played_at)
                        .unwrap_or(fallback_time);

                    sqlx::query(
                        "INSERT INTO watch_history \
                         (user_id, media_id, media_raw, event_type, session_id, play_method, position_ticks, runtime_seconds, completed, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                    )
                    .bind(user_id)
                    .bind(state.media_id)
                    .bind(&state.media_raw)
                    .bind("playback_stop")
                    .bind::<Option<String>>(None)
                    .bind::<Option<String>>(None)
                    .bind(position_ticks)
                    .bind(if runtime > 0 { Some(runtime) } else { None })
                    .bind(completed)
                    .bind(created_at)
                    .execute(&ctx.db)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to backfill watch history for user={} media={}",
                            user_id, state.media_id
                        )
                    })?;
                }
            }

            progress.report(processed, total);
            if scanned < CHUNK_SIZE as usize {
                break;
            }
            offset += CHUNK_SIZE;
        }

        Ok(())
    }
}
