-- Extend the right_media_id index to also cover left_media_id so that:
-- 1. The similar-items query (driving from media_relations filtered by genre
--    right_media_id) can resolve left_media_id without touching the table row.
-- 2. Person-filter EXISTS subqueries can seek (right_media_id, left_media_id)
--    as a single covering lookup instead of scanning all rows for a person.
DROP INDEX IF EXISTS idx_media_relations_right;
CREATE INDEX IF NOT EXISTS idx_media_relations_right_left
    ON media_relations(right_media_id, left_media_id);
