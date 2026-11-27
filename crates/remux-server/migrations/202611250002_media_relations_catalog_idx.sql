-- Catalog memberships are a small slice of media_relations (a few thousand
-- rows next to millions of cast, genre and studio links), but nothing indexes
-- them by role: `SELECT DISTINCT left_media_id ... WHERE role = 'catalog'`
-- walked the whole uniq_media_relation index on every catalog import.
-- A partial index holds only the catalog rows, so that lookup reads just those,
-- and `role = 'catalog' AND left_media_id IN (...)` resolves right_media_id
-- without touching the table. Every query on catalog rows uses the literal
-- 'catalog', which is what lets SQLite pick a partial index.
CREATE INDEX IF NOT EXISTS idx_media_relations_catalog
    ON media_relations(left_media_id, right_media_id)
    WHERE role = 'catalog';
