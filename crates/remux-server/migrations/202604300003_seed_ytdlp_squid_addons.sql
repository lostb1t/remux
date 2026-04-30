-- Seed a default yt-dlp addon if none exists.
INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at)
SELECT
    'a1b2c3d4-e5f6-4a7b-8c9d-e0f1a2b3c4d5',
    'ytdlp',
    'yt-dlp',
    '{}',
    '["streams"]',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE NOT EXISTS (SELECT 1 FROM addons WHERE kind = 'ytdlp');

-- Seed a default Squid (Tidal via monochrome) addon if none exists.
INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at)
SELECT
    'b2c3d4e5-f6a7-4b8c-9d0e-f1a2b3c4d5e6',
    'squid',
    'Squid',
    '{}',
    '["streams"]',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE NOT EXISTS (SELECT 1 FROM addons WHERE kind = 'squid');
