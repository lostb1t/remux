UPDATE user_media_state SET media_raw = NULL WHERE media_raw IS NOT NULL AND NOT json_valid(media_raw);
