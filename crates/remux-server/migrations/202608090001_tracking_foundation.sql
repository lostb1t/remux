-- Per-user addon configuration (e.g., Yamtrack token, Trakt OAuth tokens).
ALTER TABLE addon_users ADD COLUMN config TEXT NOT NULL DEFAULT '{}';

-- User ratings for media items (Jellyfin UserItemDataDto.Rating).
ALTER TABLE user_media_state ADD COLUMN rating REAL;
