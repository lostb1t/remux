ALTER TABLE iptv_sources ADD COLUMN source_type TEXT NOT NULL DEFAULT 'm3u';
ALTER TABLE iptv_sources ADD COLUMN xtream_username TEXT;
ALTER TABLE iptv_sources ADD COLUMN xtream_password TEXT;
