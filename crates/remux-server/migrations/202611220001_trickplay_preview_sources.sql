-- Streams an addon kept for seek previews only gave for a movie or episode.
-- They are not stream rows under the title, so no version list, item query or
-- play by id can reach them; only the seek preview reader opens them, through
-- the server's own stream proxy. Rows older than a day are never read.
CREATE TABLE IF NOT EXISTS trickplay_preview_sources (
    id          BLOB    NOT NULL PRIMARY KEY,
    title_id    BLOB    NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    addon_id    BLOB    NOT NULL,
    idx         INTEGER NOT NULL,
    content_key TEXT    NOT NULL,
    stream_info TEXT    NOT NULL,
    probe_data  TEXT,
    updated_at  TEXT    NOT NULL,
    UNIQUE (title_id, addon_id, content_key)
);

CREATE INDEX IF NOT EXISTS idx_trickplay_preview_sources_title_id
    ON trickplay_preview_sources(title_id);
