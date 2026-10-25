-- Per-user watchlist and ignore flags, next to `favorite`. They live in
-- user_media_state so they follow the same media_raw re-attachment on
-- purge + reimport and the same merge when items are deduplicated.
ALTER TABLE user_media_state ADD COLUMN watchlist INT NOT NULL DEFAULT 0;
ALTER TABLE user_media_state ADD COLUMN ignored INT NOT NULL DEFAULT 0;
-- Set when an item enters the watchlist; orders it by when it was added.
ALTER TABLE user_media_state ADD COLUMN watchlisted_at DATETIME;

-- Flagged rows are few per user, so the filters drive from these instead of
-- probing user_media_state once per media row.
CREATE INDEX IF NOT EXISTS idx_ums_user_watchlist
    ON user_media_state(user_id, media_id) WHERE watchlist = 1;
CREATE INDEX IF NOT EXISTS idx_ums_user_ignored
    ON user_media_state(user_id, media_id) WHERE ignored = 1;
