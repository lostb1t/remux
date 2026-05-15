-- Create media_images table
CREATE TABLE IF NOT EXISTS media_images (
    id          TEXT NOT NULL PRIMARY KEY,
    media_id    TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    image_type  TEXT NOT NULL,
    image_index INTEGER NOT NULL DEFAULT 0,
    path        TEXT NOT NULL,
    width       INTEGER,
    height      INTEGER,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (media_id, image_type, image_index)
);

CREATE INDEX IF NOT EXISTS idx_media_images_media_id ON media_images(media_id);

-- Migrate existing poster/backdrop/logo into media_images
INSERT OR IGNORE INTO media_images (id, media_id, image_type, image_index, path)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)),2) || '-' || substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6))),
       id, 'primary', 0, poster
FROM media WHERE poster IS NOT NULL;

INSERT OR IGNORE INTO media_images (id, media_id, image_type, image_index, path)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)),2) || '-' || substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6))),
       id, 'backdrop', 0, backdrop
FROM media WHERE backdrop IS NOT NULL;

INSERT OR IGNORE INTO media_images (id, media_id, image_type, image_index, path)
SELECT lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' || substr(hex(randomblob(2)),2) || '-' || substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' || hex(randomblob(6))),
       id, 'logo', 0, logo
FROM media WHERE logo IS NOT NULL;

-- Drop the now-migrated columns
ALTER TABLE media DROP COLUMN poster;
ALTER TABLE media DROP COLUMN logo;
ALTER TABLE media DROP COLUMN backdrop;
