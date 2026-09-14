-- The calendar bounds and orders items by their premiere date, expressed as
-- COALESCE(released_at, digital_released_at) — the same expression
-- ItemSortBy::PremiereDate sorts by. Neither idx_media_released_at nor
-- idx_media_digital_released_at can serve that expression, so the window scan
-- fell back to the kind index and filtered every episode row.
--
-- Measured on a 212k-item library, 761 events in the window:
--   without this index: 74 ms
--   with this index:     2 ms
-- SQLite switches from SEARCH ... USING INDEX idx_media_kind_avail_sentinel
-- (kind=?) to SEARCH ... USING INDEX (kind=? AND <expr>>? AND <expr><?).
--
-- Leading with `kind` keeps the index usable by the per-kind queries the
-- calendar issues (movies and episodes are fetched separately).
CREATE INDEX idx_media_kind_premiere
    ON media(kind, COALESCE(released_at, digital_released_at));
