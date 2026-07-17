use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct PurgeMusicTask;

#[async_trait]
impl Task for PurgeMusicTask {
    fn key(&self) -> &str {
        "PurgeMusic"
    }
    fn name(&self) -> &str {
        "Purge Music"
    }
    fn description(&self) -> &str {
        "Wipes all tracks, albums, and artists from the database."
    }
    fn short_description(&self) -> &str {
        "Removes all music items (no physical files are deleted)."
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Purge
    }
    fn destructive(&self) -> bool {
        true
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        _progress: ProgressReporter,
    ) -> Result<()> {
        let mut conn = ctx
            .db
            .acquire()
            .await?;

        sqlx::query("PRAGMA foreign_keys = OFF")
            .execute(&mut *conn)
            .await
            .ok();

        let result: Result<()> = async {
            sqlx::query("BEGIN IMMEDIATE")
                .execute(&mut *conn)
                .await?;

            sqlx::query(
                "CREATE TEMP TABLE _purge_music AS \
                 SELECT id FROM media WHERE kind IN ('track', 'album', 'artist')",
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query("CREATE INDEX _purge_music_id ON _purge_music(id)")
                .execute(&mut *conn)
                .await?;

            sqlx::query(
                "DELETE FROM media_relations \
                 WHERE left_media_id  IN (SELECT id FROM _purge_music) \
                    OR right_media_id IN (SELECT id FROM _purge_music)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "DELETE FROM media_tags WHERE media_id IN (SELECT id FROM _purge_music)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "DELETE FROM media_images WHERE media_id IN (SELECT id FROM _purge_music)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query("DELETE FROM media WHERE kind IN ('track', 'album', 'artist')")
                .execute(&mut *conn)
                .await?;

            sqlx::query("DROP TABLE _purge_music")
                .execute(&mut *conn)
                .await?;

            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await?;

            Ok(())
        }
        .await;

        sqlx::query("PRAGMA foreign_keys = ON")
            .execute(&mut *conn)
            .await
            .ok();

        result?;

        ctx.addons
            .purge_indexes(&ctx)
            .await?;

        Ok(())
    }
}
