-- Seed built-in segment addons (Probe and IntroDb) at the bottom of the addon list.
INSERT OR IGNORE INTO addons (id, name, preset, resources, types, enabled, priority, created_at, updated_at)
SELECT unhex(replace('f8f26d1f-5c53-4ae9-88db-b5465933b75a', '-', '')), 'Probe Segments', '{"kind":"probe","config":{}}',   '["segment"]', '["movie","episode"]', 1, (SELECT COALESCE(MAX(priority), 0) + 10 FROM addons), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now');
INSERT OR IGNORE INTO addons (id, name, preset, resources, types, enabled, priority, created_at, updated_at)
SELECT unhex(replace('352b899c-e4c9-4b20-b8c8-8971a3c5ec23', '-', '')), 'IntroDb',        '{"kind":"introdb","config":{}}', '["segment"]', '["episode"]',          1, (SELECT COALESCE(MAX(priority), 0) + 20 FROM addons), strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now');
