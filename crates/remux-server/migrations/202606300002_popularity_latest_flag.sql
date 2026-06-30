-- Flag column on popularity_agg: exactly one row per (media_id, period) has
-- latest = 1.  Eliminates the GROUP BY + MAX query for the popular sort —
-- the join becomes `AND pop.latest = 1` (a simple index scan).
ALTER TABLE popularity_agg ADD COLUMN latest INTEGER NOT NULL DEFAULT 0;

-- Clean up index created by the previous (deleted) migration.
DROP INDEX IF EXISTS idx_pop_agg_period_key;

-- Compute-friendly index: seek by period, then equality on latest.
CREATE INDEX IF NOT EXISTS idx_pop_agg_latest
    ON popularity_agg(period, latest, media_id);
