-- Speed up date-range filters (smart collections, digital-release gating)
CREATE INDEX IF NOT EXISTS idx_media_released_at ON media(released_at);
CREATE INDEX IF NOT EXISTS idx_media_digital_released_at ON media(digital_released_at);

-- Speed up resume/IN subquery lookups by media_id
-- (uniq_meta has kind as leading column so can't be used for media_id-only lookups)
CREATE INDEX IF NOT EXISTS idx_media_media_id ON media(media_id);
