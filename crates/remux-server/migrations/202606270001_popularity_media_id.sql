DELETE FROM popularity_raw;
DELETE FROM popularity_agg;

ALTER TABLE popularity_raw ADD COLUMN media_id BLOB;
ALTER TABLE popularity_raw ADD COLUMN media_raw TEXT;

ALTER TABLE popularity_agg ADD COLUMN media_id BLOB;
ALTER TABLE popularity_agg ADD COLUMN media_raw TEXT;

CREATE INDEX IF NOT EXISTS idx_pop_agg_media
    ON popularity_agg(media_id, source, period, period_key)
    WHERE media_id IS NOT NULL;
