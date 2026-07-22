-- Covering index for the /items/latest popularity sort.
-- The query is: WHERE period = ? AND latest = 1  ORDER BY avg DESC  LIMIT N
-- joined to media for kind/date filtering.  With this index SQLite walks rows
-- in avg-descending order and stops at LIMIT without materialising all candidates.
CREATE INDEX IF NOT EXISTS idx_pop_agg_covering
    ON popularity_agg(period, latest, avg DESC, media_id)
    WHERE latest = 1;
