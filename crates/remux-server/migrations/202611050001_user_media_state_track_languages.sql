-- The audio and subtitle language a user last chose on an item, next to the
-- raw track indexes. An index only means something inside the file it was
-- chosen in, and one title has many releases; a language still means the same
-- on the next release and the next title. Two-letter codes; subtitles also
-- take 'off'.
ALTER TABLE user_media_state ADD COLUMN audio_lang TEXT;
ALTER TABLE user_media_state ADD COLUMN subtitle_lang TEXT;

-- "The language this user chose most recently" reads the newest rows first.
CREATE INDEX IF NOT EXISTS idx_ums_user_last_played
    ON user_media_state(user_id, last_played_at DESC);
