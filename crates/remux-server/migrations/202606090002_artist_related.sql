-- Optional cache for external "related artists" (Tier 2 of the music mix).
-- When enabled, the mix expands a seed's candidate neighbourhood with the
-- artists Deezer considers related (matched back to the library by Deezer id /
-- name). Results are cached here so it is not a hot-path network call; the meta
-- table records fetch attempts (incl. empty ones) for TTL + negative caching.
-- Empty by default and never consulted unless related-artist mixing is enabled.
CREATE TABLE IF NOT EXISTS artist_related (
    artist_media_id  BLOB NOT NULL,
    related_media_id BLOB NOT NULL,
    PRIMARY KEY (artist_media_id, related_media_id)
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS artist_related_meta (
    artist_media_id BLOB PRIMARY KEY,
    fetched_at      INTEGER NOT NULL DEFAULT (unixepoch())
) WITHOUT ROWID;
