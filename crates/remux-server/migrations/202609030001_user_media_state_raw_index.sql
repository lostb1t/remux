CREATE INDEX IF NOT EXISTS idx_user_media_state_media_raw
    ON user_media_state(media_raw) WHERE media_raw IS NOT NULL;
