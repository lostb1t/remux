PRAGMA foreign_keys = ON;

CREATE TABLE users (
    id            TEXT NOT NULL PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    aio_url       TEXT,
    configuration TEXT,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    policy        TEXT
);

CREATE TABLE devices (
    user_id          TEXT NOT NULL,
    id               TEXT NOT NULL,
    access_token     TEXT NOT NULL UNIQUE,
    name             TEXT NOT NULL,
    app_name         TEXT NOT NULL,
    app_version      TEXT NOT NULL,
    last_activity_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    PRIMARY KEY (user_id, id)
);

CREATE TABLE media (
    id                       TEXT NOT NULL PRIMARY KEY,
    title                    TEXT NOT NULL,
    kind                     TEXT NOT NULL,
    imdb_id                  TEXT,
    aio_id                   TEXT,
    series_imdb_id           TEXT,
    parent_id                TEXT,
    idx                      INTEGER,
    parent_idx               INTEGER,
    released_at              TIMESTAMP,
    runtime                  INTEGER,
    rating_critic            REAL,
    rating_audience          REAL,
    certification            TEXT,
    poster                   TEXT,
    logo                     TEXT,
    backdrop                 TEXT,
    description              TEXT,
    trailers                 TEXT,
    url                      TEXT,
    probe_data               TEXT,
    remote_data              TEXT,
    promoted                 INTEGER NOT NULL DEFAULT 0,
    collection_kind          TEXT,
    collection_media_kind    TEXT,
    collection_max_items     INTEGER,
    collection_catalog_filter TEXT,
    created_at               TIMESTAMP NOT NULL,
    updated_at               TIMESTAMP NOT NULL,
    refreshed_at             TIMESTAMP,
    FOREIGN KEY (parent_id) REFERENCES media(id) ON DELETE CASCADE
);

CREATE INDEX idx_media_kind      ON media(kind);
CREATE INDEX idx_media_idx       ON media(idx);
CREATE INDEX idx_media_parent_id ON media(parent_id);

CREATE UNIQUE INDEX uniq_meta ON media (kind, aio_id)
    WHERE kind IN ('movie', 'series', 'season', 'episode');

CREATE TABLE media_relations (
    relation_id    TEXT NOT NULL PRIMARY KEY,
    left_media_id  TEXT NOT NULL,
    right_media_id TEXT NOT NULL,
    weight         INTEGER,
    role           TEXT,
    FOREIGN KEY (left_media_id)  REFERENCES media(id) ON DELETE CASCADE,
    FOREIGN KEY (right_media_id) REFERENCES media(id) ON DELETE CASCADE
);

CREATE UNIQUE INDEX uniq_media_relation
    ON media_relations (left_media_id, right_media_id, COALESCE(role, ''));

CREATE TABLE settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE api_keys (
    access_token TEXT NOT NULL PRIMARY KEY,
    app_name     TEXT NOT NULL,
    created_at   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE tasks (
    id   TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE task_triggers (
    id               TEXT NOT NULL PRIMARY KEY,
    task_id          TEXT NOT NULL,
    kind             TEXT NOT NULL,
    time_limit_hours INTEGER,
    cron             TEXT
);

CREATE INDEX idx_task_triggers_task_id ON task_triggers(task_id);

CREATE TABLE task_results (
    task_id  TEXT NOT NULL PRIMARY KEY,
    start_at DATETIME NOT NULL,
    end_at   DATETIME NOT NULL,
    status   TEXT NOT NULL
);

CREATE INDEX idx_task_results_task_id ON task_results(task_id);

CREATE TABLE jellyfin_display_prefs (
    id      TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    client  TEXT NOT NULL,
    data    TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id)
);

CREATE TABLE user_media_state (
    user_id           TEXT NOT NULL,
    media_key         TEXT NOT NULL,
    favorite          INT NOT NULL DEFAULT 0,
    play_count        INT NOT NULL DEFAULT 0,
    played_at         DATETIME,
    playback_position INT NOT NULL DEFAULT 0,
    stream_id         TEXT,
    subtitle_idx      INT,
    audio_idx         INT,
    PRIMARY KEY (user_id, media_key)
);

-- Default collections (Movies / Series smart collections)
INSERT INTO media (id, title, kind, collection_kind, collection_media_kind, promoted, created_at, updated_at)
VALUES
    (x'a1b2c3d4000040008000000000000001', 'Movies', 'collection', 'smart', 'movie',  1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'a1b2c3d4000040008000000000000002', 'Series', 'collection', 'smart', 'series', 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);

-- CatalogImport runs at startup
INSERT INTO task_triggers (id, task_id, kind, time_limit_hours, cron)
VALUES ('f47ac10b-58cc-4372-a567-0e02b2c3d479', 'CatalogImport', 'startup', NULL, NULL);
