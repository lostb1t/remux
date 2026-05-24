CREATE INDEX IF NOT EXISTS idx_media_tags_media_id ON media_tags(media_id);
-- idx_media_kind is subsumed by idx_media_kind_enabled, idx_media_kind_lower_title,
-- and idx_media_kind_available_date — all three have kind as their leading column.
DROP INDEX IF EXISTS idx_media_kind;
