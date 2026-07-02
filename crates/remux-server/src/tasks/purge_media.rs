use anyhow::Result;
use async_trait::async_trait;
use std::sync::Arc;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::AppContext;

pub struct PurgeMediaTask;

const PURGE_KINDS: &str = "'stream','movie','series','season','episode','source','track','album','artist','person','genre'";

#[async_trait]
impl Task for PurgeMediaTask {
    fn key(&self) -> &str {
        "PurgeMedia"
    }
    fn name(&self) -> &str {
        "Purge Library"
    }
    fn description(&self) -> &str {
        "Wipes all imported media from the database."
    }
    fn short_description(&self) -> &str {
        "Wipes all imported media from the database (no physical files are deleted)."
    }
    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
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
        // PASSIVE: checkpoints what it can without blocking readers or acquiring
        // an exclusive lock. TRUNCATE would hold exclusive for the full WAL flush
        // (potentially minutes after a large refresh), locking all connections.
        sqlx::query("PRAGMA wal_checkpoint(PASSIVE)")
            .execute(&ctx.db)
            .await
            .ok();

        // Acquire a dedicated connection so we can toggle foreign_keys around the
        // transaction. PRAGMA foreign_keys cannot be changed inside a transaction,
        // and the truncate optimization (O(1) DELETE FROM table with no WHERE) is
        // disabled when foreign_keys = ON.
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

            sqlx::query(&format!(
                "CREATE TEMP TABLE _keep AS \
                 SELECT * FROM media WHERE kind NOT IN ({PURGE_KINDS})"
            ))
            .execute(&mut *conn)
            .await?;
            // Index so subsequent IN (SELECT id FROM _keep) subqueries use index
            // lookups instead of full scans — critical when media_relations is large.
            sqlx::query("CREATE INDEX _keep_id ON _keep(id)")
                .execute(&mut *conn)
                .await?;

            sqlx::query(
                "CREATE TEMP TABLE _keep_images AS \
                 SELECT * FROM media_images WHERE media_id IN (SELECT id FROM _keep)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "CREATE TEMP TABLE _keep_tags AS \
                 SELECT * FROM media_tags WHERE media_id IN (SELECT id FROM _keep)",
            )
            .execute(&mut *conn)
            .await?;

            sqlx::query(
                "CREATE TEMP TABLE _keep_relations AS \
                 SELECT mr.* FROM media_relations mr \
                 INNER JOIN _keep k1 ON mr.left_media_id  = k1.id \
                 INNER JOIN _keep k2 ON mr.right_media_id = k2.id",
            )
            .execute(&mut *conn)
            .await?;

            // Snapshot media index definitions before dropping them.
            let indexes: Vec<(String, String)> = sqlx::query_as(
                "SELECT name, sql FROM sqlite_master \
                 WHERE type='index' AND tbl_name='media' AND sql IS NOT NULL",
            )
            .fetch_all(&mut *conn)
            .await?;

            for (name, _) in &indexes {
                sqlx::query(&format!("DROP INDEX IF EXISTS \"{name}\""))
                    .execute(&mut *conn)
                    .await?;
            }

            // Truncate every table — O(1) each since foreign_keys = OFF enables the
            // truncate optimization (no per-row B-tree surgery).
            sqlx::query("DELETE FROM media_tags")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DELETE FROM media_images")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DELETE FROM media_relations")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DELETE FROM media")
                .execute(&mut *conn)
                .await?;

            // Reinsert survivors and clean up temp tables.
            sqlx::query("INSERT INTO media SELECT * FROM _keep")
                .execute(&mut *conn)
                .await?;
            sqlx::query("INSERT INTO media_images SELECT * FROM _keep_images")
                .execute(&mut *conn)
                .await?;
            sqlx::query("INSERT INTO media_tags SELECT * FROM _keep_tags")
                .execute(&mut *conn)
                .await?;
            sqlx::query("INSERT INTO media_relations SELECT * FROM _keep_relations")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DROP TABLE _keep")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DROP TABLE _keep_images")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DROP TABLE _keep_tags")
                .execute(&mut *conn)
                .await?;
            sqlx::query("DROP TABLE _keep_relations")
                .execute(&mut *conn)
                .await?;

            // Rebuild media indexes over ~1,200 surviving rows — near-instant.
            for (_, sql) in &indexes {
                sqlx::query(sql)
                    .execute(&mut *conn)
                    .await?;
            }

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
