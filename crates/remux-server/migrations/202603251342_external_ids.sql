-- Add external_ids JSON column (replaces imdb_id)
ALTER TABLE media ADD COLUMN external_ids TEXT;
UPDATE media SET external_ids = json_object('imdb', imdb_id) WHERE imdb_id IS NOT NULL;

-- Add series_aio_id column (replaces series_imdb_id)
ALTER TABLE media ADD COLUMN series_aio_id TEXT;
UPDATE media SET series_aio_id = series_imdb_id WHERE series_imdb_id IS NOT NULL;

-- Drop old columns
ALTER TABLE media DROP COLUMN imdb_id;
ALTER TABLE media DROP COLUMN series_imdb_id;
