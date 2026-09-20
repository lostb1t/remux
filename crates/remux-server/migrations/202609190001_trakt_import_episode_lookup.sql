-- Trakt history resolves episodes by their series plus season/episode numbers.
-- The existing grandparent index includes parent_id before those fields, so it
-- cannot efficiently answer this lookup across every season in a series.
CREATE INDEX IF NOT EXISTS idx_media_trakt_episode_lookup
    ON media(grandparent_id, parent_idx, idx)
    WHERE kind = 'episode';
