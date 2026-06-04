-- Composite index so ORDER BY mr.left_media_id, mr.weight uses the index
-- directly instead of sorting after the left_media_id scan.
CREATE INDEX IF NOT EXISTS idx_media_relations_left_weight
    ON media_relations(left_media_id, weight);
