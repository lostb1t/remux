-- Covering index for the grandparent-scoped episode list query:
-- WHERE grandparent_id = ? AND kind IN (?) AND COALESCE(digital_released_at, released_at) <= ?
-- Supersedes idx_media_kind_grandparent (wrong column order) and
-- idx_media_grandparent_id (no kind or date).
DROP INDEX IF EXISTS idx_media_kind_grandparent;
CREATE INDEX IF NOT EXISTS idx_media_grandparent_kind_date
    ON media(grandparent_id, kind, COALESCE(digital_released_at, released_at));
ANALYZE;
