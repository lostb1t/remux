ALTER TABLE media RENAME COLUMN remote_data TO provider_info;

UPDATE media
SET provider_info = json_object('aio', json(provider_info))
WHERE provider_info IS NOT NULL
  AND json_type(provider_info, '$.aio') IS NULL;
