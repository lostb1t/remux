CREATE TABLE IF NOT EXISTS opendal_files (
    id           TEXT NOT NULL PRIMARY KEY,
    addon_id     TEXT NOT NULL REFERENCES addons(id) ON DELETE CASCADE,
    media_kind   TEXT NOT NULL,
    path         TEXT NOT NULL,
    name         TEXT NOT NULL,
    title        TEXT,
    imdb_id      TEXT,
    season       INTEGER,
    episode      INTEGER,
    track_number INTEGER,
    year         INTEGER,
    size         INTEGER,
    scanned_at   TEXT NOT NULL,
    UNIQUE(addon_id, path)
);
CREATE INDEX IF NOT EXISTS idx_opendal_files_imdb ON opendal_files(imdb_id);
CREATE INDEX IF NOT EXISTS idx_opendal_files_title ON opendal_files(addon_id, title);
