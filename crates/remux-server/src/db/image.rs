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

    /// Persist images from an addon meta refresh. Replaces an existing row only
    /// when it currently holds a provider URL (path starts with "http") and the
    /// new path differs — this lets a higher-priority addon's artwork win on
    /// every refresh while never touching a user upload or generated
    /// placeholder (local file paths). The `id` is regenerated on replace so
    /// clients, which cache images by that id, pick up the new artwork.
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
        let db = crate::db::connect("sqlite::memory:", 10_000)
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
             VALUES (?, 'x', 'movie', CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)",
        )
        .bind(id)
        .execute(db)
        .await
        .unwrap();
        id
    }

    fn primary_image(path: &str) -> MediaImages {
        MediaImages {
            primary: vec![MediaImage {
                id: Uuid::new_v4(),
                media_id: Uuid::nil(),
                image_type: "primary".into(),
                image_index: 0,
                path: path.into(),
                width: None,
                height: None,
            }],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn provider_image_replaced_but_local_upload_preserved() {
        let db = test_db().await;
        let media_id = insert_stub_media(&db).await;

        // 1) Provider image (tmdb).
        MediaImage::sync_from_media(&db, media_id, &primary_image("https://image.tmdb.org/a.jpg"))
            .await
            .unwrap();
        let before = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap();
        let id_before = before
            .get(ImageKind::Primary)
            .unwrap()
            .id;

        // 2) Refresh with a higher-priority provider (another http URL) -> must
        // replace the path AND change the id, so clients (which cache by id) see
        // the new artwork.
        MediaImage::sync_from_media(
            &db,
            media_id,
            &primary_image("https://aio.example/a.jpg"),
        )
        .await
        .unwrap();
        let after = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap();
        let p = after
            .get(ImageKind::Primary)
            .unwrap();
        assert_eq!(p.path, "https://aio.example/a.jpg");
        assert_ne!(
            p.id, id_before,
            "id must change to bust the client-side image cache"
        );

        // 3) A user upload (local path) must win over any subsequent provider sync.
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
        MediaImage::sync_from_media(
            &db,
            media_id,
            &primary_image("https://aio.example/b.jpg"),
        )
        .await
        .unwrap();
        let final_state = MediaImage::get_for_media(&db, &media_id)
            .await
            .unwrap();
        assert_eq!(
            final_state
                .get(ImageKind::Primary)
                .unwrap()
                .path,
            "/data/img/user.jpg"
        );
    }
}
