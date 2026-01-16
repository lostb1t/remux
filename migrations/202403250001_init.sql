-- Auth sessions table (SQLite)

PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS auth_devices (
    user_id      TEXT NOT NULL,
    id           TEXT NOT NULL,
    access_token TEXT NOT NULL UNIQUE,
    name  TEXT NOT NULL,
    app_name     TEXT NOT NULL,
    app_version  TEXT NOT NULL,

    PRIMARY KEY (user_id, id)
);

CREATE TABLE IF NOT EXISTS auth_users (
  id            TEXT NOT NULL PRIMARY KEY,
  username      TEXT NOT NULL UNIQUE,
  password_hash TEXT NOT NULL,
  aio_url       TEXT
);

CREATE TABLE media (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series', 'season', 'episode', 'catalog', 'source', 'unknown')),
    imdb_id TEXT,
    aio_id TEXT,
    series_imdb_id TEXT,
    parent_id TEXT,
    idx INTEGER,
    released_at TIMESTAMP,
    runtime INTEGER,
    rating_critic INTEGER,
    rating_audience INTEGER,
    poster TEXT,
    url TEXT,
    probe_data TEXT,
    remote_data TEXT,
    promoted INTEGER NOT NULL DEFAULT 0,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,

    FOREIGN KEY (parent_id) REFERENCES media(id) ON DELETE CASCADE
);


CREATE INDEX idx_media_kind ON media(kind);
CREATE INDEX idx_media_idx ON media(idx);
CREATE INDEX idx_media_parent_id ON media(parent_id);

CREATE UNIQUE INDEX uniq_media_kind_imdb
ON media (kind, aio_id)
WHERE imdb_id IS NOT NULL;