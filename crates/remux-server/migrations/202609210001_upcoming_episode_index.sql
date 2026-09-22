-- Upcoming filters and orders episodes by their air/premiere date. Keep the
-- media kind in the leading position so other kinds do not enter the scan.
CREATE INDEX IF NOT EXISTS idx_media_kind_released_at
    ON media(kind, released_at);
