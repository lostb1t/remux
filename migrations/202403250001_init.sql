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
  aio_url       TEXT NOT NULL
);

CREATE TABLE media (
    id TEXT PRIMARY KEY NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series', 'season', 'episode', 'unknown')),
    parent_id TEXT,
    imdb_id TEXT NOT NULL,
    season_num INTEGER,
    episode_num INTEGER,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL
);


CREATE TABLE provider_ids (
    media_id     TEXT    NOT NULL,
    kind  INTEGER NOT NULL,
    id        TEXT    NOT NULL,
    PRIMARY KEY (media_id, kind),
    FOREIGN KEY (media_id) REFERENCES media(id) ON DELETE CASCADE
);