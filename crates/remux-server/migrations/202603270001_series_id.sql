ALTER TABLE media ADD COLUMN series_id TEXT REFERENCES media(id);

-- Backfill: episodes → series via season
UPDATE media SET series_id = (
    SELECT s.parent_id FROM media s WHERE s.id = media.parent_id AND s.kind = 'season'
) WHERE kind = 'episode' AND series_id IS NULL;

-- Backfill: seasons → series via parent_id
UPDATE media SET series_id = parent_id
WHERE kind = 'season' AND series_id IS NULL AND parent_id IS NOT NULL;
