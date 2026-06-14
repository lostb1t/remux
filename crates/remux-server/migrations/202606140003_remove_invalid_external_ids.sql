-- Remove media rows whose external_ids JSON contains an empty string for a
-- NonEmptyString field (imdb / series_imdb). SQLx fails to decode those rows,
-- which aborts tasks like RefreshAllMeta entirely.
DELETE FROM media
WHERE json_extract(external_ids, '$.imdb') = ''
   OR json_extract(external_ids, '$.series_imdb') = '';
