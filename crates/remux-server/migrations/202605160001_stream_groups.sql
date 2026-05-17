CREATE TABLE IF NOT EXISTS stream_groups (
    id         TEXT    PRIMARY KEY NOT NULL,
    name       TEXT    NOT NULL,
    resolution TEXT,
    quality    TEXT,
    priority   INTEGER NOT NULL DEFAULT 0,
    enabled    INTEGER NOT NULL DEFAULT 1,
    created_at TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))
);
