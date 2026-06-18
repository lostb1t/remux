-- Extend idx_ums_user_play_state to include last_played_at and played_at.
-- The NextUp/ContinueWatching inner UNION selects (media_id, last_played_at, played_at)
-- filtered by (user_id, play_count/playback_position). Extending the index makes those
-- SELECTs fully covering — no table reads needed.
DROP INDEX IF EXISTS idx_ums_user_play_state;
CREATE INDEX IF NOT EXISTS idx_ums_user_play_state
    ON user_media_state(user_id, play_count, playback_position, media_id, last_played_at, played_at);

-- Fast (user_id, media_id) lookup — used by EXISTS filters and bulk user-state
-- correlated subqueries throughout db/media.rs.
CREATE INDEX IF NOT EXISTS idx_ums_user_media
    ON user_media_state(user_id, media_id, last_played_at, played_at);

-- Index for the CROSS JOIN DatePlayed records query: scan user's watches in
-- last_played_at order for early termination at LIMIT.
CREATE INDEX IF NOT EXISTS idx_ums_user_last_played
    ON user_media_state(user_id, last_played_at DESC);

-- Replace the 2-arg COALESCE index with a 3-arg sentinel version so the filter
-- COALESCE(digital_released_at, released_at, '1900-01-01 00:00:00') <= ?
-- becomes a range scan rather than a full kind-bucket scan.
DROP INDEX IF EXISTS idx_media_kind_available_date;
CREATE INDEX IF NOT EXISTS idx_media_kind_avail_sentinel
    ON media(kind, COALESCE(digital_released_at, released_at, '1900-01-01 00:00:00'));

-- Same sentinel fix for the parent_id + kind + date compound index.
DROP INDEX IF EXISTS idx_media_parent_kind_release;
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_release
    ON media(parent_id, kind, COALESCE(digital_released_at, released_at, '1900-01-01 00:00:00'));

-- Expression index for DISTINCT year query on released_at.
CREATE INDEX IF NOT EXISTS idx_media_released_year
    ON media(CAST(strftime('%Y', released_at) AS INTEGER))
    WHERE released_at IS NOT NULL;
