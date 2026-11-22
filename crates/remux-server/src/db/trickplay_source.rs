//! Streams an addon kept for seek previews only gave for a movie or episode
//! (see [`crate::trickplay`]). They are kept in a table of their own, never
//! among the title's stream rows, so no version list, automatic pick, search,
//! item query or play by id can reach them. Only the stream proxy serves one,
//! to this server's own ffmpeg and ffprobe, by its id and the server's token.

use anyhow::Result;
use chrono::NaiveDateTime;
use sqlx::SqlitePool;
use uuid::Uuid;

use crate::{api::MediaSourceInfo, db, stream::StreamInfo};

/// How long a stored answer is read. Older rows are never returned, and are
/// deleted each time an answer is stored.
pub const PREVIEW_SOURCE_TTL: chrono::Duration = chrono::Duration::hours(24);

/// One stream a previews-only addon gave for a title, in that addon's order.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PreviewSource {
    pub id: Uuid,
    pub title_id: Uuid,
    pub addon_id: Uuid,
    /// Its place in the addon's answer.
    pub idx: i64,
    /// The stream's content key, as the title's own stream rows use it.
    pub content_key: String,
    #[sqlx(json)]
    pub stream_info: StreamInfo,
    /// The addon's own media details, or a length measured here.
    #[sqlx(json(nullable))]
    pub probe_data: Option<MediaSourceInfo>,
    pub updated_at: NaiveDateTime,
}

/// What an answer holds for one stream before it is stored.
#[derive(Debug, Clone)]
pub struct NewPreviewSource {
    pub content_key: String,
    pub stream_info: StreamInfo,
    pub probe_data: Option<MediaSourceInfo>,
}

const COLS: &str =
    "id, title_id, addon_id, idx, content_key, stream_info, probe_data, updated_at";

impl PreviewSource {
    /// The id of the row a title's addon answer gives for a content key. No
    /// row of the media table has it.
    pub fn id_for(title_id: Uuid, addon_id: Uuid, content_key: &str) -> Uuid {
        Uuid::new_v5(
            &title_id,
            format!("preview:{addon_id}:{content_key}").as_bytes(),
        )
    }

    /// Stores one addon's answer for a title, in its order. A row already
    /// there for the same file keeps the length it holds; the addon's rows for
    /// the title that the answer no longer holds are deleted, and so is every
    /// row older than [`PREVIEW_SOURCE_TTL`].
    pub async fn store(
        db: &SqlitePool,
        title_id: Uuid,
        addon_id: Uuid,
        answer: &[NewPreviewSource],
        now: NaiveDateTime,
    ) -> Result<()> {
        let mut tx = db
            .begin()
            .await?;
        sqlx::query("DELETE FROM trickplay_preview_sources WHERE updated_at < ?1")
            .bind(now - PREVIEW_SOURCE_TTL)
            .execute(&mut *tx)
            .await?;
        for (idx, source) in answer
            .iter()
            .enumerate()
        {
            let mut info = source
                .stream_info
                .clone();
            // Kept in its own column, as on a stream row.
            info.probe_data = None;
            sqlx::query(
                "INSERT INTO trickplay_preview_sources \
                 (id, title_id, addon_id, idx, content_key, stream_info, probe_data, updated_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                 ON CONFLICT(id) DO UPDATE SET \
                     idx = excluded.idx, \
                     stream_info = excluded.stream_info, \
                     probe_data = COALESCE(trickplay_preview_sources.probe_data, excluded.probe_data), \
                     updated_at = excluded.updated_at",
            )
            .bind(Self::id_for(title_id, addon_id, &source.content_key))
            .bind(title_id)
            .bind(addon_id)
            .bind(idx as i64)
            .bind(&source.content_key)
            .bind(sqlx::types::Json(&info))
            .bind(
                source
                    .probe_data
                    .as_ref()
                    .map(sqlx::types::Json),
            )
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        // What the addon no longer gives for the title goes, whenever it was
        // stored.
        let kept: Vec<&str> = answer
            .iter()
            .map(|source| {
                source
                    .content_key
                    .as_str()
            })
            .collect();
        sqlx::query(
            "DELETE FROM trickplay_preview_sources \
             WHERE title_id = ?1 AND addon_id = ?2 \
               AND content_key NOT IN (SELECT value FROM json_each(?3))",
        )
        .bind(title_id)
        .bind(addon_id)
        .bind(sqlx::types::Json(&kept))
        .execute(&mut *tx)
        .await?;
        tx.commit()
            .await?;
        Ok(())
    }

    /// A title's rows younger than [`PREVIEW_SOURCE_TTL`], in each addon's
    /// order.
    pub async fn list_fresh(
        db: &SqlitePool,
        title_id: Uuid,
        now: NaiveDateTime,
    ) -> Result<Vec<Self>> {
        let rows = sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM trickplay_preview_sources \
             WHERE title_id = ?1 AND updated_at >= ?2 \
             ORDER BY addon_id, idx"
        ))
        .bind(title_id)
        .bind(now - PREVIEW_SOURCE_TTL)
        .fetch_all(db)
        .await?;
        Ok(rows)
    }

    /// The row with this id, if it is younger than [`PREVIEW_SOURCE_TTL`].
    pub async fn get_fresh(
        db: &SqlitePool,
        id: Uuid,
        now: NaiveDateTime,
    ) -> Result<Option<Self>> {
        let row = sqlx::query_as::<_, Self>(&format!(
            "SELECT {COLS} FROM trickplay_preview_sources \
             WHERE id = ?1 AND updated_at >= ?2"
        ))
        .bind(id)
        .bind(now - PREVIEW_SOURCE_TTL)
        .fetch_optional(db)
        .await?;
        Ok(row)
    }

    /// Whether the title holds rows from this addon younger than
    /// [`PREVIEW_SOURCE_TTL`].
    pub async fn has_fresh(
        db: &SqlitePool,
        title_id: Uuid,
        addon_id: Uuid,
        now: NaiveDateTime,
    ) -> Result<bool> {
        let found: Option<i64> = sqlx::query_scalar(
            "SELECT 1 FROM trickplay_preview_sources \
             WHERE title_id = ?1 AND addon_id = ?2 AND updated_at >= ?3 LIMIT 1",
        )
        .bind(title_id)
        .bind(addon_id)
        .bind(now - PREVIEW_SOURCE_TTL)
        .fetch_optional(db)
        .await?;
        Ok(found.is_some())
    }

    /// Saves a length measured here on the row. Its age does not change.
    pub async fn save_probe_data(
        db: &SqlitePool,
        id: Uuid,
        probe: &MediaSourceInfo,
    ) -> Result<()> {
        sqlx::query(
            "UPDATE trickplay_preview_sources SET probe_data = ?1 WHERE id = ?2",
        )
        .bind(sqlx::types::Json(probe))
        .bind(id)
        .execute(db)
        .await?;
        Ok(())
    }

    /// The row as a stream row in memory, under its title, so the checks and
    /// the set of a title's own versions apply to it unchanged. It is never
    /// saved as one.
    pub fn to_stream_row(&self, title_runtime: Option<i64>) -> db::Media {
        let mut info = self
            .stream_info
            .clone();
        info.probe_data = None;
        db::Media {
            id: self.id,
            title: info
                .name
                .clone()
                .or_else(|| {
                    info.description
                        .clone()
                })
                .unwrap_or_default(),
            kind: db::MediaKind::Stream,
            parent_id: Some(self.title_id),
            idx: Some(self.idx),
            runtime: title_runtime,
            stream_info: Some(info),
            probe_data: self
                .probe_data
                .clone(),
            created_at: self.updated_at,
            updated_at: self.updated_at,
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stream::StreamDescriptor;

    async fn pool() -> SqlitePool {
        let pool = db::connect("sqlite::memory:", 10_000)
            .await
            .unwrap();
        db::migrate(&pool)
            .await
            .unwrap();
        pool
    }

    async fn title(pool: &SqlitePool) -> Uuid {
        // A film is saved under an outside id; each one here gets its own.
        static NEXT: std::sync::atomic::AtomicI64 =
            std::sync::atomic::AtomicI64::new(1);
        let mut film = db::Media {
            title: "Invented Film".to_string(),
            kind: db::MediaKind::Movie,
            external_ids: db::ExternalIds {
                tmdb: Some(NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)),
                ..Default::default()
            },
            ..Default::default()
        };
        film.save(pool)
            .await
            .unwrap();
        film.id
    }

    fn answer(keys: &[&str]) -> Vec<NewPreviewSource> {
        keys.iter()
            .map(|key| NewPreviewSource {
                content_key: key.to_string(),
                stream_info: StreamInfo {
                    descriptor: StreamDescriptor::http(&format!(
                        "http://example.invalid/{key}.mkv"
                    )),
                    ..Default::default()
                },
                probe_data: None,
            })
            .collect()
    }

    fn keys(rows: &[PreviewSource]) -> Vec<String> {
        rows.iter()
            .map(|row| {
                row.content_key
                    .clone()
            })
            .collect()
    }

    #[tokio::test]
    async fn an_answer_is_stored_in_order_and_replaces_the_last() {
        let pool = pool().await;
        let film = title(&pool).await;
        let addon = Uuid::new_v4();
        let now = chrono::Utc::now().naive_utc();
        PreviewSource::store(&pool, film, addon, &answer(&["b", "a", "c"]), now)
            .await
            .unwrap();
        let rows = PreviewSource::list_fresh(&pool, film, now)
            .await
            .unwrap();
        assert_eq!(keys(&rows), ["b", "a", "c"]);
        assert_eq!(rows[0].id, PreviewSource::id_for(film, addon, "b"));
        assert!(
            PreviewSource::has_fresh(&pool, film, addon, now)
                .await
                .unwrap()
        );
        assert!(
            !PreviewSource::has_fresh(&pool, film, Uuid::new_v4(), now)
                .await
                .unwrap()
        );

        // A measured length stays with its file; a file the new answer no
        // longer holds goes.
        let measured = MediaSourceInfo {
            run_time_ticks: Some(7_200 * 10_000_000),
            ..Default::default()
        };
        PreviewSource::save_probe_data(&pool, rows[1].id, &measured)
            .await
            .unwrap();
        let later = now + chrono::Duration::minutes(5);
        PreviewSource::store(&pool, film, addon, &answer(&["a", "d"]), later)
            .await
            .unwrap();
        let rows = PreviewSource::list_fresh(&pool, film, later)
            .await
            .unwrap();
        assert_eq!(keys(&rows), ["a", "d"]);
        assert_eq!(
            rows[0]
                .probe_data
                .as_ref()
                .and_then(|p| p.run_time_ticks),
            Some(7_200 * 10_000_000)
        );

        // An empty answer leaves the addon nothing for the title.
        PreviewSource::store(&pool, film, addon, &[], later)
            .await
            .unwrap();
        assert!(
            PreviewSource::list_fresh(&pool, film, later)
                .await
                .unwrap()
                .is_empty()
        );
    }

    #[tokio::test]
    async fn nothing_older_than_a_day_is_read_and_a_new_answer_deletes_it() {
        let pool = pool().await;
        let film = title(&pool).await;
        let other = title(&pool).await;
        let addon = Uuid::new_v4();
        let then = chrono::Utc::now().naive_utc() - chrono::Duration::hours(25);
        PreviewSource::store(&pool, film, addon, &answer(&["a"]), then)
            .await
            .unwrap();
        let id = PreviewSource::id_for(film, addon, "a");
        let now = chrono::Utc::now().naive_utc();
        assert!(
            PreviewSource::get_fresh(&pool, id, then)
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            PreviewSource::get_fresh(&pool, id, now)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            PreviewSource::list_fresh(&pool, film, now)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !PreviewSource::has_fresh(&pool, film, addon, now)
                .await
                .unwrap()
        );
        // Any title's new answer deletes it.
        PreviewSource::store(&pool, other, addon, &answer(&["b"]), now)
            .await
            .unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trickplay_preview_sources")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 1);
    }

    #[tokio::test]
    async fn rows_go_with_their_title() {
        let pool = pool().await;
        let film = title(&pool).await;
        let now = chrono::Utc::now().naive_utc();
        PreviewSource::store(&pool, film, Uuid::new_v4(), &answer(&["a", "b"]), now)
            .await
            .unwrap();
        sqlx::query("DELETE FROM media WHERE id = ?1")
            .bind(film)
            .execute(&pool)
            .await
            .unwrap();
        let left: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM trickplay_preview_sources")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(left, 0);
    }

    #[test]
    fn a_row_reads_as_a_stream_under_its_title() {
        let film = Uuid::new_v4();
        let addon = Uuid::new_v4();
        let row = PreviewSource {
            id: PreviewSource::id_for(film, addon, "a"),
            title_id: film,
            addon_id: addon,
            idx: 3,
            content_key: "a".to_string(),
            stream_info: StreamInfo {
                name: Some("Invented release".to_string()),
                addon_id: Some(addon),
                ..Default::default()
            },
            probe_data: None,
            updated_at: chrono::Utc::now().naive_utc(),
        };
        let stream = row.to_stream_row(Some(7_200));
        assert_eq!(stream.kind, db::MediaKind::Stream);
        assert_eq!(stream.parent_id, Some(film));
        assert_eq!(stream.idx, Some(3));
        assert_eq!(stream.runtime, Some(7_200));
        assert_eq!(stream.title, "Invented release");
        assert_ne!(stream.id, film);
    }
}
