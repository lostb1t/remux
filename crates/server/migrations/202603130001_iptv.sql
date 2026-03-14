-- IPTV sources (M3U + EPG URLs)
CREATE TABLE IF NOT EXISTS iptv_sources (
    id               TEXT NOT NULL PRIMARY KEY,
    name             TEXT NOT NULL,
    m3u_url          TEXT NOT NULL,
    epg_url          TEXT,
    refresh_interval TEXT NOT NULL DEFAULT '24h'
);

-- Add IPTV-specific columns to media (channels & programs stored as regular media items)
ALTER TABLE media ADD COLUMN live_start   TEXT;
ALTER TABLE media ADD COLUMN live_end     TEXT;
ALTER TABLE media ADD COLUMN tvg_id       TEXT;
ALTER TABLE media ADD COLUMN channel_number INTEGER;
