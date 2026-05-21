-- Guard: create table with current schema if it somehow doesn't exist yet,
-- so the UPDATE below doesn't fail on fresh databases.
CREATE TABLE IF NOT EXISTS stream_groups (
    id         TEXT    PRIMARY KEY NOT NULL,
    name       TEXT    NOT NULL,
    filter     TEXT,
    priority   INTEGER NOT NULL DEFAULT 0,
    enabled    INTEGER NOT NULL DEFAULT 1,
    hidden     INTEGER NOT NULL DEFAULT 0,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);

UPDATE stream_groups
SET filter = replace(filter, '"field":"source"', '"field":"quality"')
WHERE filter LIKE '%"field":"source"%';

UPDATE media
SET stream_group_data = replace(stream_group_data, '"field":"source"', '"field":"quality"')
WHERE stream_group_data LIKE '%"field":"source"%';
