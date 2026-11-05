-- The audio and subtitle language a user last chose on an item, next to the
-- raw track indexes. An index only means something inside the file it was
-- chosen in, and one title has many releases; a language still means the same
-- on the next release and the next title. Two-letter codes; subtitles also
-- take 'off'. Each carries when it was chosen: `last_played_at` moves on every
-- replay, which is not a choice.
ALTER TABLE user_media_state ADD COLUMN audio_lang TEXT;
ALTER TABLE user_media_state ADD COLUMN audio_lang_at DATETIME;
ALTER TABLE user_media_state ADD COLUMN subtitle_lang TEXT;
ALTER TABLE user_media_state ADD COLUMN subtitle_lang_at DATETIME;

-- "The language this user chose most recently" reads the newest choice.
CREATE INDEX IF NOT EXISTS idx_ums_user_audio_lang
    ON user_media_state(user_id, audio_lang_at DESC) WHERE audio_lang IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_ums_user_subtitle_lang
    ON user_media_state(user_id, subtitle_lang_at DESC) WHERE subtitle_lang IS NOT NULL;
