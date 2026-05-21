-- Episodes: move series IMDB from external_ids.imdb → external_ids.series_imdb.
-- Fill from the grandparent series row to ensure consistency.
UPDATE media
SET external_ids = json_set(
    json_remove(external_ids, '$.imdb'),
    '$.series_imdb',
    (SELECT json_extract(ser.external_ids, '$.imdb') FROM media ser WHERE ser.id = media.grandparent_id)
)
WHERE kind = 'episode'
  AND json_extract(external_ids, '$.imdb') IS NOT NULL;

-- Seasons: fill series_imdb from parent series (seasons have empty external_ids).
UPDATE media
SET external_ids = json_set(
    external_ids,
    '$.series_imdb',
    (SELECT json_extract(ser.external_ids, '$.imdb') FROM media ser WHERE ser.id = media.parent_id)
)
WHERE kind = 'season'
  AND (SELECT json_extract(ser.external_ids, '$.imdb') FROM media ser WHERE ser.id = media.parent_id) IS NOT NULL;

ALTER TABLE media DROP COLUMN grandparent_media_id;
