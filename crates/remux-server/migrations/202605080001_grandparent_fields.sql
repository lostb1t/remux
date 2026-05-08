ALTER TABLE media RENAME COLUMN series_id TO grandparent_id;
ALTER TABLE media RENAME COLUMN series_media_id TO grandparent_media_id;
DROP INDEX IF EXISTS idx_media_series_id;
CREATE INDEX IF NOT EXISTS idx_media_grandparent_id ON media(grandparent_id);
