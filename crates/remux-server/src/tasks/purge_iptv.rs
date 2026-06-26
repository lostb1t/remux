use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskService};
use crate::AppContext;

pub struct PurgeIptvTask;

#[async_trait]
impl Task for PurgeIptvTask {
    fn key(&self) -> &str {
        "PurgeIptv"
    }
    fn name(&self) -> &str {
        "Purge IPTV"
    }
    fn description(&self) -> &str {
        "Wipes all IPTV channels and programs from the database."
    }
    fn short_description(&self) -> &str {
        "Removes all TV channels and programs (no physical files are deleted)."
    }
    fn category(&self) -> &str {
        "Live TV"
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
        // Acquire a dedicated connection so we can toggle foreign_keys around the
        // transaction. PRAGMA foreign_keys cannot be changed inside a transaction,
        // and disabling it lets us delete relations explicitly without cascade overhead.
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
                "CREATE TEMP TABLE _purge_iptv AS \
                 SELECT id FROM media WHERE kind IN ('tv_channel', 'tv_program')",
            )
            .execute(&mut *conn)
            .await?;
            sqlx::query("CREATE INDEX _purge_iptv_id ON _purge_iptv(id)")
                .execute(&mut *conn)
                .await?;

            // left_media_id has no FK/cascade — must delete those orphans explicitly.
            sqlx::query(
                "DELETE FROM media_relations \
                 WHERE left_media_id  IN (SELECT id FROM _purge_iptv) \
                    OR right_media_id IN (SELECT id FROM _purge_iptv)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "DELETE FROM media_tags WHERE media_id IN (SELECT id FROM _purge_iptv)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "DELETE FROM media_images WHERE media_id IN (SELECT id FROM _purge_iptv)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query("DELETE FROM media WHERE kind IN ('tv_channel', 'tv_program')")
                .execute(&mut *conn)
                .await?;

            sqlx::query("DROP TABLE _purge_iptv")
                .execute(&mut *conn)
                .await?;

            sqlx::query("COMMIT")
                .execute(&mut *conn)
                .await?;

            Ok(())
        }
        .await;

        // Always restore FK before returning the connection to the pool.
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
