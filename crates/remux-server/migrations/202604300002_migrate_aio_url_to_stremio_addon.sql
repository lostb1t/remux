-- Migrate the server-wide aio_url from the settings JSON blob into a Stremio
-- addon row so it keeps working after the addons refactor. No-op when:
--   (a) no stremio addon exists yet but aio_url is missing/empty/"not-configured"
--   (b) a stremio addon already exists (manual config wins)
INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at)
SELECT
    lower(hex(randomblob(4))) || '-' ||
    lower(hex(randomblob(2))) || '-4' ||
    substr(lower(hex(randomblob(2))), 2) || '-' ||
    substr('89ab', abs(random()) % 4 + 1, 1) ||
    substr(lower(hex(randomblob(2))), 2) || '-' ||
    lower(hex(randomblob(6))),
    'stremio',
    'AIO',
    json_object('manifest_url', json_extract(value, '$.aio_url')),
    '["catalog","meta","search","subtitles","streams"]',
    10,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
FROM settings
WHERE key = 'server_configuration'
  AND json_extract(value, '$.aio_url') IS NOT NULL
  AND json_extract(value, '$.aio_url') != ''
  AND json_extract(value, '$.aio_url') != 'http://not-configured'
  AND NOT EXISTS (SELECT 1 FROM addons WHERE kind = 'stremio');
