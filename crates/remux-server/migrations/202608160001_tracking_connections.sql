-- A user's connection to one tracking addon.
--
-- Deliberately not addon_users: that table is the per-user addon *source
-- override* list, so a row there restricts which addons the user can play
-- from, and replacing the override set would drop the credentials with it.
CREATE TABLE tracking_connections (
    id              BLOB PRIMARY KEY NOT NULL,
    addon_id        BLOB NOT NULL REFERENCES addons(id) ON DELETE CASCADE,
    user_id         BLOB NOT NULL REFERENCES users(id)  ON DELETE CASCADE,
    -- disconnected | connected | error | auth_expired
    status          TEXT NOT NULL DEFAULT 'disconnected',
    -- Opaque provider-defined JSON: a webhook token, or an OAuth triple.
    credentials     TEXT NOT NULL DEFAULT '{}',
    -- JSON array of TrackingEventKind. Empty means send nothing.
    event_filters   TEXT NOT NULL DEFAULT '[]',
    last_success_at DATETIME,
    last_error_at   DATETIME,
    last_error      TEXT,
    -- retryable | permanent
    last_error_kind TEXT,
    created_at      DATETIME NOT NULL,
    updated_at      DATETIME NOT NULL,
    UNIQUE (addon_id, user_id)
);

CREATE INDEX idx_tracking_connections_user_id ON tracking_connections(user_id);
CREATE INDEX idx_tracking_connections_addon_id ON tracking_connections(addon_id);
