-- Rename aio_id → media_id and series_aio_id → series_media_id.
-- These columns serve as a provider-agnostic content GUID, not specifically an AIO ID.
ALTER TABLE media RENAME COLUMN aio_id TO media_id;
ALTER TABLE media RENAME COLUMN series_aio_id TO series_media_id;

-- Recreate the unique index under the new column name.
DROP INDEX IF EXISTS uniq_meta;
CREATE UNIQUE INDEX uniq_meta ON media (kind, media_id);
