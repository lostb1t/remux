-- Migrate existing iptv_sources rows into the addons table as iptv-m3u or iptv-xtream presets.
-- The addon UUID is preserved (same bytes as the old iptv_sources.id) so that
-- iptv_source_id values stored in media.external_ids remain valid without any update.

INSERT OR IGNORE INTO addons (id, name, preset, resources, types, enabled, priority, created_at, updated_at)
SELECT
    -- Convert id to BLOB if stored as TEXT, otherwise pass through as-is.
    CASE typeof(s.id)
        WHEN 'text' THEN unhex(replace(s.id, '-', ''))
        ELSE s.id
    END,
    s.name,
    CASE s.source_type
        WHEN 'xtream' THEN json_object(
            'kind', 'iptv-xtream',
            'config', json_object(
                'server_url', s.m3u_url,
                'username', COALESCE(s.xtream_username, ''),
                'password', COALESCE(s.xtream_password, '')
            )
        )
        ELSE json_object(
            'kind', 'iptv-m3u',
            'config', json_object('url', s.m3u_url)
        )
    END,
    '["stream","catalog"]',
    '["tv_channel"]',
    1,
    0,
    datetime('now'),
    datetime('now')
FROM iptv_sources s;

DROP TABLE IF EXISTS iptv_sources;
