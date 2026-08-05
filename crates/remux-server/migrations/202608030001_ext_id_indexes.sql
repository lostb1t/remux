-- Child deduplication: Season / Episode / Track by (parent_id, kind, idx).
-- SQLite does not auto-index FK columns.
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_idx
    ON media(parent_id, kind, idx)
    WHERE parent_id IS NOT NULL;

-- Grandchild preload: used by the existing_l2 query in process_meta_item when
-- a root UUID is adopted and grandchildren need UUID adoption too.
CREATE INDEX IF NOT EXISTS idx_media_grandparent_kind_idx
    ON media(grandparent_id, parent_id, kind, idx)
    WHERE grandparent_id IS NOT NULL;
