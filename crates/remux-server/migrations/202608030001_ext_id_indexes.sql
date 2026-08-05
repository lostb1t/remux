-- Child deduplication: Season / Episode / Track by (parent_id, kind, idx).
-- SQLite does not auto-index FK columns.
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_idx
    ON media(parent_id, kind, idx)
    WHERE parent_id IS NOT NULL;
