-- Simkl is an admin-managed, per-user history source. Connected account
-- histories are combined by its catalog implementation when Refresh Library
-- runs; this row gives that runtime a stable ID.
INSERT OR IGNORE INTO addons (
    id, name, preset, resources, types, enabled, priority, created_at, updated_at,
    system, is_default, http_redirect_stream, service_filter
) VALUES (
    X'73696d6b6c0000000000000000000001',
    'Simkl',
    '{"kind":"simkl","config":{}}',
    '["catalog","meta"]',
    '["movie","series"]',
    1,
    0,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP,
    1,
    1,
    0,
    '[]'
);
