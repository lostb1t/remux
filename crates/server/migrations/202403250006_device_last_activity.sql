ALTER TABLE devices
    ADD COLUMN last_activity_at TEXT NOT NULL
    DEFAULT (strftime('%Y-%m-%dT%H:%M:%SZ', 'now'));
