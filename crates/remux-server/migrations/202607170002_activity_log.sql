-- Server activity/audit log, backing Jellyfin's `GET /System/ActivityLog/Entries`
-- (surfaced by the admin dashboard's Activity page). Populated from real events:
-- logins, playback start/stop, scheduled-task failures, and user create/delete.
CREATE TABLE IF NOT EXISTS activity_log (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    name           TEXT    NOT NULL,
    overview       TEXT,
    short_overview TEXT,
    type           TEXT    NOT NULL,
    item_id        TEXT,
    user_id        TEXT,
    date           TEXT    NOT NULL, -- ISO8601 UTC
    severity       TEXT    NOT NULL DEFAULT 'Information' -- LogLevel
);

-- Entries are read newest-first and optionally filtered by user presence.
CREATE INDEX IF NOT EXISTS idx_activity_log_date ON activity_log (date DESC);
CREATE INDEX IF NOT EXISTS idx_activity_log_user ON activity_log (user_id);
