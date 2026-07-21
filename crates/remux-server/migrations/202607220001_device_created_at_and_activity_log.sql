ALTER TABLE devices ADD COLUMN created_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));

CREATE TABLE IF NOT EXISTS activity_log (
    id          TEXT    PRIMARY KEY,
    timestamp   TEXT    NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now')),
    user_id     TEXT    NOT NULL,
    user_name   TEXT    NOT NULL DEFAULT '',
    action      TEXT    NOT NULL,
    target_user_id   TEXT,
    target_user_name TEXT,
    device_id   TEXT,
    device_name TEXT,
    details     TEXT
);

CREATE INDEX IF NOT EXISTS idx_activity_log_timestamp ON activity_log (timestamp DESC);
