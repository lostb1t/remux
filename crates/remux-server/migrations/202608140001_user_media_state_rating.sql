-- Personal rating, matching Jellyfin's UserItemData.Rating: a 0-10 double.
-- `Likes` is not stored separately; Jellyfin derives it from this value at a
-- 6.5 threshold, and its /Rating endpoint writes 10 or 1 through that.
ALTER TABLE user_media_state ADD COLUMN rating REAL;
