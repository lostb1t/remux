-- Prefix existing catalog media_id values with "aio:" to namespace them by provider.
-- Guards against running twice with NOT LIKE 'aio:%'.
UPDATE media
SET media_id = 'aio:' || media_id
WHERE kind = 'catalog'
  AND media_id IS NOT NULL
  AND media_id NOT LIKE 'aio:%';
