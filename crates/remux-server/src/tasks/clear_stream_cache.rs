use anyhow::Result;
use async_trait::async_trait;
use sqlx::SqlitePool;
use std::sync::Arc;
use tracing::info;

use super::{ProgressReporter, Task, TaskCategory, TaskService};
use crate::{AppContext, db};

pub struct ClearStreamCacheTask;

#[async_trait]
impl Task for ClearStreamCacheTask {
    fn key(&self) -> &str {
        "ClearStreamCache"
    }

    fn name(&self) -> &str {
        "Clear Stream Cache"
    }

    fn description(&self) -> &str {
        "Deletes all cached stream lists. Streams are fetched again when a media item is opened."
    }

    fn short_description(&self) -> &str {
        "Deletes all cached stream lists"
    }

    fn category(&self) -> TaskCategory {
        TaskCategory::Maintenance
    }

    async fn run(
        &self,
        ctx: AppContext,
        _tasks: Arc<TaskService>,
        progress: ProgressReporter,
    ) -> Result<()> {
        let (removed, invalidated) = clear_stream_cache(&ctx.db).await?;
        info!(removed, invalidated, "cleared stream cache");
        progress.set(100.0);
        Ok(())
    }
}

async fn clear_stream_cache(db: &SqlitePool) -> Result<(u64, u64)> {
    let mut tx = db
        .begin()
        .await?;

    let removed = sqlx::query("DELETE FROM media WHERE kind = ?")
        .bind(db::MediaKind::Stream)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let invalidated = sqlx::query(
        "UPDATE media SET streams_refreshed_at = NULL WHERE streams_refreshed_at IS NOT NULL",
    )
    .execute(&mut *tx)
    .await?
    .rows_affected();

    tx.commit()
        .await?;
    Ok((removed, invalidated))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use uuid::Uuid;

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    async fn insert_media(
        db: &SqlitePool,
        id: Uuid,
        kind: db::MediaKind,
        parent_id: Option<Uuid>,
        streams_refreshed: bool,
    ) {
        let now = Utc::now().naive_utc();
        sqlx::query(
            "INSERT INTO media \
             (id, title, kind, parent_id, created_at, updated_at, streams_refreshed_at) \
             VALUES (?, 'Test', ?, ?, ?, ?, ?)",
        )
        .bind(id)
        .bind(kind)
        .bind(parent_id)
        .bind(now)
        .bind(now)
        .bind(streams_refreshed.then_some(now))
        .execute(db)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn removes_all_streams_and_invalidates_every_parent() {
        let db = test_db().await;
        let movie_id = Uuid::new_v4();
        let episode_id = Uuid::new_v4();

        insert_media(&db, movie_id, db::MediaKind::Movie, None, true).await;
        insert_media(&db, episode_id, db::MediaKind::Episode, None, true).await;
        insert_media(
            &db,
            Uuid::new_v4(),
            db::MediaKind::Stream,
            Some(movie_id),
            false,
        )
        .await;
        insert_media(
            &db,
            Uuid::new_v4(),
            db::MediaKind::Stream,
            Some(episode_id),
            false,
        )
        .await;

        let (removed, invalidated) = clear_stream_cache(&db)
            .await
            .unwrap();

        assert_eq!(removed, 2);
        assert_eq!(invalidated, 2);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM media WHERE kind = ?")
                .bind(db::MediaKind::Stream)
                .fetch_one(&db)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM media WHERE streams_refreshed_at IS NOT NULL",
            )
            .fetch_one(&db)
            .await
            .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>(
                "SELECT COUNT(*) FROM media WHERE id IN (?, ?)"
            )
            .bind(movie_id)
            .bind(episode_id)
            .fetch_one(&db)
            .await
            .unwrap(),
            2
        );
    }
}
