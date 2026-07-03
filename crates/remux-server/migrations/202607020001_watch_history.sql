-- Track playback and watch activity separately from playback state.
-- This table is intentionally append-only and keyed for historical analysis.
CREATE TABLE IF NOT EXISTS watch_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    user_id           BLOB    NOT NULL,
    media_id          BLOB    NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    media_raw         TEXT,
    event_type        TEXT    NOT NULL,
    session_id        TEXT,
    play_method       TEXT,
    position_ticks    INTEGER NOT NULL DEFAULT 0,
    runtime_seconds   INTEGER,
    completed         INTEGER NOT NULL DEFAULT 0,
    created_at        DATETIME NOT NULL DEFAULT CURRENT_TIMESTAMP
);

CREATE INDEX IF NOT EXISTS idx_watch_history_user_media_created
    ON watch_history(user_id, media_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_watch_history_user_created
    ON watch_history(user_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_watch_history_media_created
    ON watch_history(media_id, created_at DESC);
