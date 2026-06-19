-- Restore the iptv_sources table that was accidentally dropped during the
-- migration squash in PR #92 (commit cf8efd1). The table pre-dated the squash
-- but was not re-created there, while db::IptvSource (src/db/iptv.rs) kept
-- querying it — so every IPTV source operation (list/get/save/delete) failed
-- with "no such table: iptv_sources" on databases initialized from the squash.

-- Columns mirror the IptvSource struct in src/db/iptv.rs:
--   id, name, m3u_url, epg_url (deprecated, kept for compatibility),
--   refresh_interval, source_type ('m3u' | 'xtream'),
--   xtream_username, xtream_password.
CREATE TABLE IF NOT EXISTS iptv_sources (
    id               TEXT NOT NULL PRIMARY KEY,
    name             TEXT NOT NULL,
    m3u_url          TEXT NOT NULL,
    epg_url          TEXT,
    refresh_interval TEXT NOT NULL DEFAULT '24h',
    source_type      TEXT NOT NULL DEFAULT 'm3u',
    xtream_username  TEXT,
    xtream_password  TEXT
);
