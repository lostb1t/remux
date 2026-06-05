-- Composite index for the common "list children of a parent, filtered by kind" query:
-- SELECT * FROM media WHERE parent_id = ? AND kind = 'episode'
-- Used by /Shows/{id}/Episodes and /Shows/{id}/Seasons.
CREATE INDEX IF NOT EXISTS idx_media_parent_kind ON media(parent_id, kind);
