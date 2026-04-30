-- Seed a default TMDB addon if none exists yet.
INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at)
SELECT
    'df84acaa-fe34-4fe7-b826-15599646062e',
    'tmdb',
    'TMDB',
    '{}',
    '["meta","search"]',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE NOT EXISTS (SELECT 1 FROM addons WHERE kind = 'tmdb');

-- Seed a default Deezer addon if none exists yet.
INSERT INTO addons (id, kind, name, config, resources, priority, created_at, updated_at)
SELECT
    '384bb706-9919-46af-8875-38f669909b90',
    'deezer',
    'Deezer',
    '{"playlists":[]}',
    '["catalog","meta","search"]',
    0,
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now'),
    strftime('%Y-%m-%dT%H:%M:%SZ', 'now')
WHERE NOT EXISTS (SELECT 1 FROM addons WHERE kind = 'deezer');
