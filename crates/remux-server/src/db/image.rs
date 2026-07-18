use std::{collections::HashMap, str::FromStr};

use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use uuid::Uuid;

/// The four image types stored in `media_images`.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Serialize,
    Deserialize,
    strum_macros::Display,
    strum_macros::EnumString,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case", ascii_case_insensitive)]
pub enum ImageKind {
    Primary,
    Backdrop,
    Logo,
    Thumb,
}

/// A single image row from `media_images`.
#[derive(Debug, Clone, sqlx::FromRow, Serialize, Deserialize)]
pub struct MediaImage {
    pub id: Uuid,
    pub media_id: Uuid,
    pub image_type: String,
    pub image_index: i64,
    pub path: String,
    pub width: Option<i64>,
    pub height: Option<i64>,
}

impl MediaImage {
    /// Get images for a single media item.
    pub async fn get_for_media(
        db: &SqlitePool,
        media_id: &Uuid,
    ) -> Result<MediaImages, sqlx::Error> {
        let rows = sqlx::query_as::<_, Self>(
            "SELECT id, media_id, image_type, image_index, path, width, height \
             FROM media_images WHERE media_id = ? ORDER BY image_type, image_index",
        )
        .bind(media_id)
        .fetch_all(db)
        .await?;
        Ok(MediaImages::from(rows))
    }

    /// Batch-load images for a set of media IDs.
    pub async fn get_for_media_ids(
        db: &SqlitePool,
        ids: &[Uuid],
    ) -> Result<HashMap<Uuid, MediaImages>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let mut qb = sqlx::QueryBuilder::new(
            "SELECT id, media_id, image_type, image_index, path, width, height \
             FROM media_images WHERE media_id IN (",
        );
        let mut sep = qb.separated(", ");
        for id in ids {
            sep.push_bind(id);
        }
        qb.push(") ORDER BY media_id, image_type, image_index");
        let rows = qb
            .build_query_as::<Self>()
            .fetch_all(db)
            .await?;
        let mut flat: HashMap<Uuid, Vec<Self>> = HashMap::new();
        for row in rows {
            flat.entry(row.media_id)
                .or_default()
                .push(row);
        }
        Ok(flat
            .into_iter()
            .map(|(k, v)| (k, MediaImages::from(v)))
            .collect())
    }

    /// Persist addon-sourced images. Replaces an existing row only when the
    /// stored path is a provider URL (starts with "http") and the new path
    /// differs — so a higher-priority addon's artwork wins on every refresh
    /// while user uploads and generated placeholders (local paths) are never
    /// touched. The `id` is regenerated on replace so clients that cache by id
    /// pick up the new artwork.
    pub async fn sync_from_media(
        db: &SqlitePool,
        media_id: Uuid,
        images: &MediaImages,
    ) -> Result<(), sqlx::Error> {
        for image in images.iter() {
            sqlx::query(
                "INSERT INTO media_images \
                 (id, media_id, image_type, image_index, path, width, height) \
                 VALUES (?, ?, ?, ?, ?, ?, ?) \
                 ON CONFLICT (media_id, image_type, image_index) DO UPDATE SET \
                   id = excluded.id, \
                   path = excluded.path, \
                   width = excluded.width, \
                   height = excluded.height \
                 WHERE media_images.path LIKE 'http%' \
                   AND media_images.path <> excluded.path",
            )
            .bind(Uuid::new_v4())
            .bind(media_id)
            .bind(&image.image_type)
            .bind(image.image_index)
            .bind(&image.path)
            .bind(image.width)
            .bind(image.height)
            .execute(db)
            .await?;
        }
        Ok(())
    }

    /// User-upload save: INSERT OR REPLACE with a fresh UUID (busts image cache).
    pub async fn save(
        db: &SqlitePool,
        media_id: Uuid,
        kind: ImageKind,
        path: &str,
        width: Option<i64>,
        height: Option<i64>,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT OR REPLACE INTO media_images \
             (id, media_id, image_type, image_index, path, width, height) \
             VALUES (?, ?, ?, 0, ?, ?, ?)",
        )
        .bind(Uuid::new_v4())
        .bind(media_id)
        .bind(kind.to_string())
        .bind(path)
        .bind(width)
        .bind(height)
        .execute(db)
        .await?;
        Ok(())
    }

    /// Delete a single image type for a media item (index 0).
    pub async fn delete_for_type(
        db: &SqlitePool,
        media_id: Uuid,
        kind: ImageKind,
    ) -> Result<(), sqlx::Error> {
        sqlx::query(
            "DELETE FROM media_images \
             WHERE media_id = ? AND image_type = ? AND image_index = 0",
        )
        .bind(media_id)
        .bind(kind.to_string())
        .execute(db)
        .await?;
        Ok(())
    }
}

/// Typed image collection for a media item.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MediaImages {
    pub primary: Vec<MediaImage>,
    pub backdrop: Vec<MediaImage>,
    pub logo: Vec<MediaImage>,
    pub thumb: Vec<MediaImage>,
}

impl MediaImages {
    /// Return the first image of the given kind at index 0, if any.
    pub fn get(&self, kind: ImageKind) -> Option<&MediaImage> {
        let vec = match kind {
            ImageKind::Primary => &self.primary,
            ImageKind::Backdrop => &self.backdrop,
            ImageKind::Logo => &self.logo,
            ImageKind::Thumb => &self.thumb,
        };
        vec.iter()
            .find(|i| i.image_index == 0)
    }

    /// Return the path for the first image of the given kind at index 0.
    pub fn get_path(&self, kind: ImageKind) -> Option<&str> {
        self.get(kind)
            .map(|i| {
                i.path
                    .as_str()
            })
    }

    pub fn is_empty(&self) -> bool {
        self.primary
            .is_empty()
            && self
                .backdrop
                .is_empty()
            && self
                .logo
                .is_empty()
            && self
                .thumb
                .is_empty()
    }

    /// Iterate over all images across all types.
    pub fn iter(&self) -> impl Iterator<Item = &MediaImage> {
        self.primary
            .iter()
            .chain(
                self.backdrop
                    .iter(),
            )
            .chain(
                self.logo
                    .iter(),
            )
            .chain(
                self.thumb
                    .iter(),
            )
    }
}

impl IntoIterator for MediaImages {
    type Item = MediaImage;
    type IntoIter = std::vec::IntoIter<MediaImage>;

    fn into_iter(self) -> Self::IntoIter {
        let mut all = self.primary;
        all.extend(self.backdrop);
        all.extend(self.logo);
        all.extend(self.thumb);
        all.into_iter()
    }
}

impl From<Vec<MediaImage>> for MediaImages {
    fn from(images: Vec<MediaImage>) -> Self {
        let mut result = MediaImages::default();
        for img in images {
            match ImageKind::from_str(&img.image_type) {
                Ok(ImageKind::Primary) => result
                    .primary
                    .push(img),
                Ok(ImageKind::Backdrop) => result
                    .backdrop
                    .push(img),
                Ok(ImageKind::Logo) => result
                    .logo
                    .push(img),
                Ok(ImageKind::Thumb) => result
                    .thumb
                    .push(img),
                Err(_) => {}
            }
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> SqlitePool {
        let db = crate::db::connect("sqlite::memory:", 10_000, 5)
            .await
            .unwrap();
        crate::db::migrate(&db)
            .await
            .unwrap();
        db
    }

    async fn insert_stub_media(db: &SqlitePool) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO media (id, title, kind, created_at, updated_at) \
             VALUES (?, 'stub', 'movie', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(id)
        .execute(db)
        .await
        .unwrap();
        id
    }

    fn primary(path: &str) -> MediaImages {
        MediaImages {
            primary: vec![MediaImage {
                id: Uuid::new_v4(),
                media_id: Uuid::nil(),
                image_type: ImageKind::Primary.to_string(),
                image_index: 0,
                path: path.into(),
                width: None,
                height: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn provider_url_replaced_by_higher_priority_provider() {
        let db = test_db().await;
        let media_id = insert_stub_media(&db).await;

        // Insert initial provider image
        MediaImage::sync_from_media(&db, media_id, &primary("https://tmdb.org/a.jpg"))
            .await
            .unwrap();
        let before = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap();
        let id_before = before
            .get(ImageKind::Primary)
            .unwrap()
            .id;

        // Higher-priority addon provides a different URL → must replace
        MediaImage::sync_from_media(
            &db,
            media_id,
            &primary("https://aio.example/a.jpg"),
        )
        .await
        .unwrap();
        let after = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap();
        let img = after
            .get(ImageKind::Primary)
            .unwrap();
        assert_eq!(img.path, "https://aio.example/a.jpg");
        // id must change so clients bust their image cache
        assert_ne!(img.id, id_before);
    }

    #[tokio::test]
    async fn same_provider_url_not_updated() {
        let db = test_db().await;
        let media_id = insert_stub_media(&db).await;

        MediaImage::sync_from_media(&db, media_id, &primary("https://tmdb.org/a.jpg"))
            .await
            .unwrap();
        let id_first = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap()
            .get(ImageKind::Primary)
            .unwrap()
            .id;

        // Same URL again → no update, id stays the same
        MediaImage::sync_from_media(&db, media_id, &primary("https://tmdb.org/a.jpg"))
            .await
            .unwrap();
        let id_second = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap()
            .get(ImageKind::Primary)
            .unwrap()
            .id;

        assert_eq!(id_first, id_second);
    }

    #[tokio::test]
    async fn local_upload_not_overwritten_by_provider() {
        let db = test_db().await;
        let media_id = insert_stub_media(&db).await;

        // User uploads a local image
        MediaImage::save(
            &db,
            media_id,
            ImageKind::Primary,
            "/data/img/user.jpg",
            None,
            None,
        )
        .await
        .unwrap();

        // Provider sync must not overwrite it
        MediaImage::sync_from_media(
            &db,
            media_id,
            &primary("https://aio.example/a.jpg"),
        )
        .await
        .unwrap();

        let path = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap()
            .get(ImageKind::Primary)
            .unwrap()
            .path
            .clone();
        assert_eq!(path, "/data/img/user.jpg");
    }
}
