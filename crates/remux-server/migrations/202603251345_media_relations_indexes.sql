-- Add missing index on media_relations.right_media_id.
-- Without this, every DELETE on media triggers a full table scan of media_relations
-- to resolve the ON DELETE CASCADE, making bulk deletes O(n²).
CREATE INDEX IF NOT EXISTS idx_media_relations_right ON media_relations(right_media_id);

-- Also index media_relations.left_media_id for symmetric lookup performance.
CREATE INDEX IF NOT EXISTS idx_media_relations_left ON media_relations(left_media_id);
