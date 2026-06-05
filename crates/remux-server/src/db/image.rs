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

    /// Persist images using INSERT OR IGNORE
    /// (preserves existing UUID — addon re-imports don't bust the cache).
    pub async fn sync_from_media(
        db: &SqlitePool,
        media_id: Uuid,
        images: &MediaImages,
    ) -> Result<(), sqlx::Error> {
        for image in images.iter() {
            sqlx::query(
                "INSERT OR IGNORE INTO media_images \
                 (id, media_id, image_type, image_index, path, width, height) \
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
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
