-- Indexes that speed up bulk deletes in the PurgeMedia task.
-- media_images(media_id): the explicit pre-delete subquery was full-scanning this table.
-- media(parent_id/grandparent_id): SQLite checks these for self-referential FK cascade on every deleted row.
-- media_catalog_items(media_id): purge task didn't pre-delete from this table; cascade was scanning it.
CREATE INDEX IF NOT EXISTS idx_media_images_media_id ON media_images(media_id);
CREATE INDEX IF NOT EXISTS idx_media_parent_id ON media(parent_id);
CREATE INDEX IF NOT EXISTS idx_media_grandparent_id ON media(grandparent_id);
CREATE INDEX IF NOT EXISTS idx_media_catalog_items_media_id ON media_catalog_items(media_id);
ANALYZE;
