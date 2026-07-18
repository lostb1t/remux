-- ---- Tables ------------------------------------------------------------------

CREATE TABLE IF NOT EXISTS users (
    id            TEXT NOT NULL PRIMARY KEY,
    username      TEXT NOT NULL UNIQUE,
    password_hash TEXT NOT NULL,
    aio_url       TEXT,
    configuration TEXT,
    is_admin      INTEGER NOT NULL DEFAULT 0,
    policy        TEXT
);

CREATE TABLE IF NOT EXISTS devices (
    user_id          TEXT    NOT NULL,
    id               TEXT    NOT NULL,
    access_token     TEXT    NOT NULL UNIQUE,
    name             TEXT    NOT NULL,
    app_name         TEXT    NOT NULL,
    app_version      TEXT    NOT NULL,
    last_activity_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    capabilities     TEXT,
    remote_ip        TEXT,
    PRIMARY KEY (user_id, id)
);

CREATE TABLE IF NOT EXISTS api_keys (
    access_token TEXT     NOT NULL PRIMARY KEY,
    app_name     TEXT     NOT NULL,
    created_at   DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE TABLE IF NOT EXISTS settings (
    key   TEXT NOT NULL PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS jellyfin_display_prefs (
    id      TEXT NOT NULL PRIMARY KEY,
    user_id TEXT NOT NULL,
    client  TEXT NOT NULL,
    data    TEXT NOT NULL,
    FOREIGN KEY (user_id) REFERENCES users(id) ON DELETE CASCADE
);

CREATE TABLE IF NOT EXISTS media (
    id                              TEXT      NOT NULL PRIMARY KEY,
    title                           TEXT      NOT NULL,
    kind                            TEXT      NOT NULL,
    parent_id                       TEXT      REFERENCES media(id) ON DELETE CASCADE,
    grandparent_id                  TEXT      REFERENCES media(id),
    idx                             INTEGER,
    parent_idx                      INTEGER,
    released_at                     TIMESTAMP,
    digital_released_at             TIMESTAMP,
    runtime                         INTEGER,
    rating_critic                   REAL,
    rating_audience                 REAL,
    external_ratings                TEXT,
    certification                   TEXT,
    certification_age               INTEGER,
    description                     TEXT,
    trailers                        TEXT,
    probe_data                      TEXT,
    stream_info                     TEXT,
    stream_group_data               TEXT,
    status                          TEXT,
    country                         TEXT,
    external_ids                    TEXT,
    program_kind                    TEXT,
    promoted                        INTEGER   NOT NULL DEFAULT 0,
    enabled                         INTEGER   NOT NULL DEFAULT 1,
    sort_order                      INTEGER,
    custom_name                     TEXT,
    live_start                      TEXT,
    live_end                        TEXT,
    tvg_id                          TEXT,
    channel_number                  INTEGER,
    collection_kind                 TEXT,
    collection_media_kind           TEXT,
    collection_max_items            INTEGER,
    collection_smart_filter         TEXT,
    collection_source               TEXT,
    collection_default_sort         TEXT,
    collection_default_sort_order   TEXT,
    collection_latest_auto_unplayed BOOLEAN,
    collection_latest_sort_digital  BOOLEAN,
    created_at                      TIMESTAMP NOT NULL,
    updated_at                      TIMESTAMP NOT NULL,
    refreshed_at                    TIMESTAMP,
    streams_refreshed_at            DATETIME
);

CREATE INDEX IF NOT EXISTS idx_media_idx                  ON media(idx);
CREATE INDEX IF NOT EXISTS idx_media_parent_id            ON media(parent_id);
CREATE INDEX IF NOT EXISTS idx_media_grandparent_id       ON media(grandparent_id);
CREATE INDEX IF NOT EXISTS idx_media_title                ON media(title COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_media_released_at          ON media(released_at);
CREATE INDEX IF NOT EXISTS idx_media_digital_released_at  ON media(digital_released_at);
CREATE INDEX IF NOT EXISTS idx_media_live_end             ON media(live_end);
CREATE INDEX IF NOT EXISTS idx_media_kind_enabled         ON media(kind, enabled);
CREATE INDEX IF NOT EXISTS idx_media_kind_grandparent     ON media(kind, grandparent_id);
CREATE INDEX IF NOT EXISTS idx_media_kind_lower_title     ON media(kind, lower(title));
CREATE INDEX IF NOT EXISTS idx_media_kind_available_date  ON media(kind, COALESCE(digital_released_at, released_at));
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_release  ON media(parent_id, kind, COALESCE(digital_released_at, released_at));
CREATE INDEX IF NOT EXISTS idx_media_id_kind_gp           ON media(id, kind, grandparent_id);

CREATE TABLE IF NOT EXISTS media_tags (
    media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    tag      TEXT NOT NULL COLLATE NOCASE,
    PRIMARY KEY (media_id, tag COLLATE NOCASE)
);

CREATE INDEX IF NOT EXISTS idx_media_tags_tag      ON media_tags(tag COLLATE NOCASE);
CREATE INDEX IF NOT EXISTS idx_media_tags_media_id ON media_tags(media_id);

CREATE TABLE IF NOT EXISTS media_relations (
    relation_id    TEXT NOT NULL PRIMARY KEY,
    left_media_id  TEXT NOT NULL,
    right_media_id TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    weight         INTEGER,
    role           TEXT,
    character      TEXT
);

CREATE UNIQUE INDEX IF NOT EXISTS uniq_media_relation
    ON media_relations (left_media_id, right_media_id, COALESCE(role, ''));

CREATE TABLE IF NOT EXISTS media_images (
    id          TEXT     NOT NULL PRIMARY KEY,
    media_id    TEXT     NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    image_type  TEXT     NOT NULL,
    image_index INTEGER  NOT NULL DEFAULT 0,
    path        TEXT     NOT NULL,
    width       INTEGER,
    height      INTEGER,
    created_at  DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE (media_id, image_type, image_index)
);

CREATE INDEX IF NOT EXISTS idx_media_images_media_id ON media_images(media_id);

-- user_id is intentionally not a FOREIGN KEY — watch state/favorites are
-- preserved even after user deletion (e.g. for re-creation or historical stats).
CREATE TABLE IF NOT EXISTS user_media_state (
    user_id           BLOB     NOT NULL,
    media_id          BLOB     NOT NULL,
    media_raw         TEXT,
    favorite          INT      NOT NULL DEFAULT 0,
    play_count        INT      NOT NULL DEFAULT 0,
    played_at         DATETIME,
    playback_position INT      NOT NULL DEFAULT 0,
    stream_id         BLOB,
    subtitle_idx      INT,
    audio_idx         INT,
    last_played_at    DATETIME,
    PRIMARY KEY (user_id, media_id)
);

CREATE INDEX IF NOT EXISTS idx_ums_user_play_state
    ON user_media_state(user_id, play_count, playback_position, media_id);

CREATE TABLE IF NOT EXISTS addons (
    id         BLOB    NOT NULL PRIMARY KEY,
    name       TEXT    NOT NULL,
    preset     TEXT    NOT NULL DEFAULT '{"kind":"","config":{}}',
    resources  TEXT    NOT NULL DEFAULT '[]',
    types      TEXT    NOT NULL DEFAULT '[]',
    enabled    INTEGER NOT NULL DEFAULT 1,
    priority   INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL,
    updated_at TEXT    NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_addons_preset_kind ON addons(json_extract(preset, '$.kind'));

CREATE TABLE IF NOT EXISTS opendal_files (
    id           TEXT    NOT NULL PRIMARY KEY,
    addon_id     TEXT    NOT NULL REFERENCES addons(id) ON DELETE CASCADE,
    media_kind   TEXT    NOT NULL,
    path         TEXT    NOT NULL,
    name         TEXT    NOT NULL,
    title        TEXT,
    imdb_id      TEXT,
    season       INTEGER,
    episode      INTEGER,
    track_number INTEGER,
    year         INTEGER,
    size         INTEGER,
    scanned_at   TEXT    NOT NULL,
    UNIQUE(addon_id, path)
);

CREATE INDEX IF NOT EXISTS idx_opendal_files_imdb  ON opendal_files(imdb_id);
CREATE INDEX IF NOT EXISTS idx_opendal_files_title ON opendal_files(addon_id, title);

CREATE TABLE IF NOT EXISTS stream_groups (
    id         TEXT    PRIMARY KEY NOT NULL,
    name       TEXT    NOT NULL,
    filter     TEXT,
    priority   INTEGER NOT NULL DEFAULT 0,
    enabled    INTEGER NOT NULL DEFAULT 1,
    hidden     INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

CREATE TABLE IF NOT EXISTS epg_sources (
    id               TEXT NOT NULL PRIMARY KEY,
    name             TEXT NOT NULL,
    url              TEXT NOT NULL,
    refresh_interval TEXT NOT NULL DEFAULT '24h'
);

CREATE TABLE IF NOT EXISTS tasks (
    id   TEXT NOT NULL PRIMARY KEY,
    name TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS task_triggers (
    id               TEXT    NOT NULL PRIMARY KEY,
    task_id          TEXT    NOT NULL,
    kind             TEXT    NOT NULL,
    time_limit_hours INTEGER,
    cron             TEXT
);

CREATE INDEX IF NOT EXISTS idx_task_triggers_task_id ON task_triggers(task_id);

CREATE TABLE IF NOT EXISTS task_results (
    task_id  TEXT     NOT NULL PRIMARY KEY,
    start_at DATETIME NOT NULL,
    end_at   DATETIME NOT NULL,
    status   TEXT     NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_task_results_task_id ON task_results(task_id);

-- ---- Seed data ---------------------------------------------------------------

-- Default library view collections
INSERT OR IGNORE INTO media (id, title, kind, collection_kind, collection_media_kind, collection_smart_filter, collection_default_sort, collection_default_sort_order, collection_latest_auto_unplayed, collection_latest_sort_digital, external_ids, promoted, enabled, sort_order, created_at, updated_at) VALUES
    (x'247c646d1e9d4b9e88b159bfc2924515', 'Popular Movies',     'collection', 'smart',  'movie',      '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["PopularityDay"]',      '["Descending"]', 1, 0, '{}', 1, 1,  1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'1c48a70fa69c40b9b69de3f76211c3e5', 'Popular Shows',      'collection', 'smart',  'series',     '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["PopularityDay"]',      '["Ascending"]',  1, 0, '{}', 1, 1,  2, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'cf064b931c0d58af8a8f0adf4009e989', 'Trending Movies',    'collection', 'smart',  'movie',      '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["TrendingWeek"]',       '["Descending"]', 1, 0, '{}', 1, 1,  3, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'9945cf7b406f551e80a90b298a2ff392', 'Trending Shows',     'collection', 'smart',  'series',     '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["TrendingWeek"]',       '["Descending"]', 1, 0, '{}', 1, 1,  4, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'f47ac10b58cc4372a5670e02b2c3d479', 'Collections',        'collection', 'manual', 'collection', '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          NULL,                     NULL,             NULL, NULL, '{}', 1, 1,  5, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'a1b2c3d4000040008000000000000001', 'Movies',             'collection', 'smart',  'movie',      '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["DigitalReleaseDate"]', '["Descending"]', 1, 1, '{}', 1, 1,  6, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'a1b2c3d4000040008000000000000002', 'Shows',              'collection', 'smart',  'series',     '{"match_mode":"all","groups":[{"match_mode":"all","rules":[]}]}',                                                                                          '["DigitalReleaseDate"]', '["Descending"]', 1, 1, '{}', 1, 1,  7, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'211d58cdf8504c74b4866f15480ebcc3', 'Playlists',          'collection', 'smart',  'playlist',   NULL,                                                                                                                                                        NULL,                     NULL,             NULL, NULL, '{}', 1, 1,  8, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'a1b2c3d4000040008000000000000003', 'Music',              'collection', 'smart',  'music',      NULL,                                                                                                                                                        NULL,                     NULL,             NULL, NULL, '{}', 1, 1,  9, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'f46d2385bf6f4ad689aa38232323731a', 'Top Netflix Movies', 'collection', 'smart',  'movie',      '{"match_mode":"all","groups":[{"match_mode":"all","rules":[{"field":"tag","op":"in","values":["provider:Netflix"]}]}]}', '["PopularityDay"]',      '["Descending"]', 0, 0, '{}', 0, 1, 10, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'2f73513185424c75a57a4073e532e833', 'Top Netflix Shows',  'collection', 'smart',  'series',     '{"match_mode":"all","groups":[{"match_mode":"all","rules":[{"field":"tag","op":"in","values":["provider:Netflix"]}]}]}', '["PopularityDay"]',      '["Descending"]', 0, 0, '{}', 0, 1, 11, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);

-- Default task triggers
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron) VALUES
    ('a1b2c3d4-e5f6-7890-abcd-ef1234567890', 'CleanTranscodeFolder', 'IntervalTrigger', NULL, '0 0 */24 * * *'),
    ('default-refreshlibrary-daily',          'RefreshLibrary',       'DailyTrigger',    NULL, '0 0 4 * * *'),
    ('default-refreshlibrary-startup',        'RefreshLibrary',       'StartupTrigger',  NULL, NULL);

-- Default addons
INSERT OR IGNORE INTO addons (id, name, preset, resources, types, enabled, priority, created_at, updated_at)
SELECT unhex(replace('df84acaa-fe34-4fe7-b826-15599646062e', '-', '')), 'TMDB',    '{"kind":"tmdb","config":{"catalogs":{"popular_movies":{"enabled":true,"max_items":null,"tags":[]},"popular_tv":{"enabled":true,"max_items":null,"tags":[]},"top_rated_movies":{"enabled":true,"max_items":null,"tags":[]},"top_rated_tv":{"enabled":true,"max_items":null,"tags":[]},"trending_movies_week":{"enabled":true,"max_items":null,"tags":[]},"trending_tv_week":{"enabled":true,"max_items":null,"tags":[]}}}}', '["search","meta","catalog","metrics"]', '[]', 1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now') UNION ALL
SELECT unhex(replace('384bb706-9919-46af-8875-38f669909b90', '-', '')), 'Deezer',  '{"kind":"deezer","config":{"playlists":[]}}', '["catalog","meta","search"]', '[]', 1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now') UNION ALL
SELECT unhex(replace('6d492b8a-6d42-4c32-aff0-bc695c5e582a', '-', '')), 'Monochrome', '{"kind":"monochrome","config":{}}',           '["search","stream","catalog"]','[]', 1, 0, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'), strftime('%Y-%m-%dT%H:%M:%SZ', 'now');
