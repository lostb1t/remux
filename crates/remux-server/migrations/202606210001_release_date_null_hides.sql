-- Switch the release-date filter from 3-arg COALESCE (sentinel '1900-01-01' makes
-- null-date items always pass) to 2-arg COALESCE (null-date items have NULL in the
-- index, so NULL <= ? evaluates to NULL/false and they are naturally hidden).
--
-- This means: when FilterByDigitalReleaseDate is on, content with no release date
-- is treated as "not yet released" rather than "released in 1900".

DROP INDEX IF EXISTS idx_media_kind_avail_sentinel;
CREATE INDEX IF NOT EXISTS idx_media_kind_avail_sentinel
    ON media(kind, COALESCE(digital_released_at, released_at));

DROP INDEX IF EXISTS idx_media_parent_kind_release;
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_release
    ON media(parent_id, kind, COALESCE(digital_released_at, released_at));
