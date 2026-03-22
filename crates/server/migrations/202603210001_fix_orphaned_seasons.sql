-- Re-link orphaned seasons to their correct series.
-- Matches on aio_id (= imdb_id for series) to use the uniq_meta index.
UPDATE media
SET parent_id = (
    SELECT id FROM media s
    WHERE s.kind = 'series' AND s.aio_id = media.series_imdb_id
    LIMIT 1
)
WHERE kind = 'season'
  AND series_imdb_id IS NOT NULL;

-- Re-link orphaned episodes to their correct season.
-- Season aio_id is "<series_imdb_id>:<season_idx>", also covered by uniq_meta.
UPDATE media
SET parent_id = (
    SELECT id FROM media s
    WHERE s.kind = 'season'
      AND s.aio_id = (media.series_imdb_id || ':' || media.parent_idx)
    LIMIT 1
)
WHERE kind = 'episode'
  AND series_imdb_id IS NOT NULL
  AND parent_idx IS NOT NULL;
