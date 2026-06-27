-- Add "metrics" to TMDB addon's resources array if not already present.
UPDATE addons
SET resources = json_insert(resources, '$[#]', 'metrics')
WHERE id = unhex(replace('df84acaa-fe34-4fe7-b826-15599646062e', '-', ''))
  AND NOT EXISTS (
    SELECT 1 FROM json_each(resources) WHERE value = 'metrics'
  );

-- Rebuild popularity_agg without source/external_id, preserving existing data.
-- Per-source rows for the same media_id are merged by averaging, matching
-- what the new rollup queries produce going forward.
CREATE TABLE popularity_agg_new (
    media_id     BLOB    NOT NULL,
    period       TEXT    NOT NULL,
    period_key   TEXT    NOT NULL,
    avg          REAL    NOT NULL,
    min          REAL    NOT NULL,
    max          REAL    NOT NULL,
    sample_count INTEGER NOT NULL,
    PRIMARY KEY (media_id, period, period_key)
);

INSERT INTO popularity_agg_new (media_id, period, period_key, avg, min, max, sample_count)
SELECT media_id, period, period_key, AVG(avg), MIN(min), MAX(max), SUM(sample_count)
FROM popularity_agg
WHERE media_id IS NOT NULL
GROUP BY media_id, period, period_key;

DROP TABLE popularity_agg;
ALTER TABLE popularity_agg_new RENAME TO popularity_agg;

CREATE INDEX IF NOT EXISTS idx_pop_agg_lookup
    ON popularity_agg(media_id, period, period_key);
