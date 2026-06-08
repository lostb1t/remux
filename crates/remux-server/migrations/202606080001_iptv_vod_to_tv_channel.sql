-- IPTV VOD movies and series were incorrectly stored as kind='movie'/'series'.
-- Convert them to kind='tv_channel' with the appropriate program_kind so they
-- stay in the IPTV catalog and are no longer picked up by metadata refresh.

UPDATE media
SET kind = 'tv_channel', program_kind = 'movie'
WHERE kind = 'movie'
  AND json_extract(external_ids, '$.iptv_source_id') IS NOT NULL;

UPDATE media
SET kind = 'tv_channel', program_kind = 'series'
WHERE kind = 'series'
  AND json_extract(external_ids, '$.iptv_source_id') IS NOT NULL;
