CREATE INDEX IF NOT EXISTS idx_media_kind_lower_title ON media(kind, lower(title));
