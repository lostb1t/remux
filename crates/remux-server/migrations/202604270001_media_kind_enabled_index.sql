CREATE INDEX IF NOT EXISTS idx_media_kind_enabled ON media(kind, enabled);
CREATE INDEX IF NOT EXISTS idx_media_live_end ON media(live_end);
