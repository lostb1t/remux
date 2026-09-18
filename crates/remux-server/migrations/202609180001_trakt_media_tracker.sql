-- Trakt is a built-in media-tracker provider. Application credentials remain
-- in Config; this row only gives the generic addon runtime a stable identity.
INSERT OR IGNORE INTO addons (
    id, name, preset, resources, types, enabled, priority, created_at, updated_at,
    system, is_default, http_redirect_stream, service_filter
) VALUES (
    X'7472616b740000000000000000000001',
    'Trakt',
    '{"kind":"trakt","config":{}}',
    '[]',
    '[]',
    1,
    0,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP,
    1,
    1,
    0,
    '[]'
);

ALTER TABLE user_media_trackers ADD COLUMN remote_account_id TEXT;
ALTER TABLE user_media_trackers ADD COLUMN remote_account_name TEXT;

CREATE TABLE media_tracker_auth_attempts (
    id                    BLOB PRIMARY KEY NOT NULL,
    user_id               BLOB NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    addon_id              BLOB NOT NULL REFERENCES addons(id) ON DELETE CASCADE,
    poll_token            TEXT NOT NULL,
    poll_interval_seconds INTEGER NOT NULL,
    next_poll_at          DATETIME NOT NULL,
    expires_at            DATETIME NOT NULL,
    created_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX idx_media_tracker_auth_attempts_expiry
    ON media_tracker_auth_attempts(expires_at);

CREATE TABLE media_tracker_import_runs (
    id                    BLOB PRIMARY KEY NOT NULL,
    user_media_tracker_id BLOB NOT NULL REFERENCES user_media_trackers(id) ON DELETE CASCADE,
    status                TEXT NOT NULL,
    fetched_count         INTEGER NOT NULL DEFAULT 0,
    matched_count         INTEGER NOT NULL DEFAULT 0,
    updated_count         INTEGER NOT NULL DEFAULT 0,
    deferred_count        INTEGER NOT NULL DEFAULT 0,
    skipped_count         INTEGER NOT NULL DEFAULT 0,
    error                 TEXT,
    created_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    started_at            DATETIME,
    completed_at          DATETIME
);

CREATE INDEX idx_media_tracker_import_runs_tracker
    ON media_tracker_import_runs(user_media_tracker_id, created_at DESC);

CREATE TABLE media_tracker_outbox (
    id                    BLOB PRIMARY KEY NOT NULL,
    user_media_tracker_id BLOB NOT NULL REFERENCES user_media_trackers(id) ON DELETE CASCADE,
    session_id            TEXT NOT NULL DEFAULT '',
    event_kind            TEXT NOT NULL,
    event_json            TEXT NOT NULL,
    target_json           TEXT NOT NULL,
    dedupe_key            TEXT NOT NULL,
    status                TEXT NOT NULL DEFAULT 'pending',
    attempts              INTEGER NOT NULL DEFAULT 0,
    next_attempt_at       DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    last_error            TEXT,
    created_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at            DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(user_media_tracker_id, dedupe_key)
);

CREATE INDEX idx_media_tracker_outbox_due
    ON media_tracker_outbox(status, next_attempt_at, created_at);

CREATE INDEX idx_media_tracker_outbox_tracker_session
    ON media_tracker_outbox(user_media_tracker_id, session_id, created_at);
