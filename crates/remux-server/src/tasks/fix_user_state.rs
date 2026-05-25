use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;
use tracing::{debug, info, warn};
use uuid::Uuid;

use super::{ProgressReporter, Task, TaskService};
use crate::{AppContext, db};

pub struct FixUserStateTask;

#[async_trait]
impl Task for FixUserStateTask {
    fn key(&self) -> &str {
        "FixUserState"
    }
    fn name(&self) -> &str {
        "Fix User State"
    }
    fn description(&self) -> &str {
        "Recomputes stable UUIDs for existing user media state rows whose media_id is a random UUID. Rows with a media_raw JSON field are fixed; rows without usable external IDs are left as-is."
    }
    fn short_description(&self) -> &str {
        "Fixes broken media_id references in watch history and favorites."
    }
    fn category(&self) -> &str {
        "Maintenance"
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        // Fetch all rows whose media_raw looks like a MediaIdRaw JSON object.
        let rows: Vec<(Uuid, Uuid, String)> = sqlx::query_as(
            "SELECT user_id, media_id, media_raw FROM user_media_state WHERE media_raw LIKE '{%'",
        )
        .fetch_all(&ctx.db)
        .await?;

        info!(total = rows.len(), "scanning user_media_state rows");

        let mut fixed = 0u32;
        let mut skipped = 0u32;

        for (user_id, old_id, raw_json) in rows {
            let Ok(raw) = serde_json::from_str::<db::MediaIdRaw>(&raw_json) else {
                debug!(%user_id, %old_id, "media_raw is not a valid MediaIdRaw, skipping");
                skipped += 1;
                continue;
            };

            let stable_id = Uuid::from(&raw);
            if stable_id == old_id {
                continue; // already correct
            }

            // Fetch the full broken row.
            let old_row: Option<db::UserMediaState> = sqlx::query_as(
                "SELECT * FROM user_media_state WHERE user_id = ? AND media_id = ?",
            )
            .bind(user_id)
            .bind(old_id)
            .fetch_optional(&ctx.db)
            .await?;

            let Some(old_row) = old_row else {
                continue; // disappeared between fetch and now
            };

            // Check if a correct row (stable_id) already exists.
            let existing: Option<db::UserMediaState> = sqlx::query_as(
                "SELECT * FROM user_media_state WHERE user_id = ? AND media_id = ?",
            )
            .bind(user_id)
            .bind(stable_id)
            .fetch_optional(&ctx.db)
            .await?;

            let merged = if let Some(ex) = existing {
                // Merge: prefer the row with more play history for position.
                let use_old_pos = old_row.play_count >= ex.play_count;
                db::UserMediaState {
                    user_id,
                    media_id: stable_id,
                    media_raw: old_row.media_raw.clone(),
                    favorite: old_row.favorite || ex.favorite,
                    play_count: old_row.play_count.max(ex.play_count),
                    played_at: max_datetime(old_row.played_at, ex.played_at),
                    playback_position: if use_old_pos {
                        old_row.playback_position
                    } else {
                        ex.playback_position
                    },
                    last_played_at: max_datetime(
                        old_row.last_played_at,
                        ex.last_played_at,
                    ),
                    subtitle_idx: old_row.subtitle_idx.or(ex.subtitle_idx),
                    audio_idx: old_row.audio_idx.or(ex.audio_idx),
                    stream_id: old_row.stream_id.or(ex.stream_id),
                }
            } else {
                db::UserMediaState {
                    media_id: stable_id,
                    ..old_row.clone()
                }
            };

            // Delete old row(s) and insert merged row in a transaction.
            let result: Result<()> = async {
                sqlx::query("BEGIN").execute(&ctx.db).await?;
                sqlx::query(
                    "DELETE FROM user_media_state WHERE user_id = ? AND media_id IN (?, ?)",
                )
                .bind(user_id)
                .bind(old_id)
                .bind(stable_id)
                .execute(&ctx.db)
                .await?;
                merged.save(&ctx.db).await?;
                sqlx::query("COMMIT").execute(&ctx.db).await?;
                Ok(())
            }
            .await;

            match result {
                Ok(()) => {
                    debug!(%user_id, old = %old_id, new = %stable_id, "fixed media_id");
                    fixed += 1;
                }
                Err(e) => {
                    warn!(%user_id, %old_id, error = %e, "failed to fix row, rolling back");
                    sqlx::query("ROLLBACK").execute(&ctx.db).await.ok();
                    skipped += 1;
                }
            }
        }

        info!(fixed, skipped, "Fix User State complete");
        Ok(())
    }
}

fn max_datetime(
    a: Option<chrono::NaiveDateTime>,
    b: Option<chrono::NaiveDateTime>,
) -> Option<chrono::NaiveDateTime> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}
