-- Covering index for the parent-scoped episode/season list query.
-- Includes the COALESCE expression so the planner can satisfy the full WHERE clause
-- (parent_id = ?, kind = ?, COALESCE(digital_released_at, released_at) <= ?)
-- from a single B-tree range scan without touching the main table.
-- Supersedes idx_media_parent_kind (which lacks the date expression and has no stats).
DROP INDEX IF EXISTS idx_media_parent_kind;
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_release
    ON media(parent_id, kind, COALESCE(digital_released_at, released_at));
ANALYZE;
