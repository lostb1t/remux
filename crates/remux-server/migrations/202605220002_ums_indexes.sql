-- Recreate play-state index lost when user_media_state was rebuilt in 202605210003.
-- Covers resumable queries  (user_id, play_count=0, playback_position>0 → media_id)
-- and nextup queries        (user_id, play_count>0                        → media_id)
CREATE INDEX IF NOT EXISTS idx_ums_user_play_state
    ON user_media_state(user_id, play_count, playback_position, media_id);
