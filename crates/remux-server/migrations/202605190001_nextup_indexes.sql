-- Allow efficient lookup in user_media_state BY media_key.
-- Without this, JOIN/NOT EXISTS queries that drive from the media side do a
-- full ums scan per episode row. Covering play_count+playback_position means
-- the OR/EXISTS conditions can be checked in-index without fetching the row.
CREATE INDEX IF NOT EXISTS idx_ums_media_key
  ON user_media_state(media_key, user_id, play_count, playback_position);

-- Allow efficient filtering by (user_id, play_count, playback_position) in the
-- resumable IN-subquery in get_by_filter, without range-scanning all of a
-- user's states and post-filtering.
CREATE INDEX IF NOT EXISTS idx_ums_user_play_state
  ON user_media_state(user_id, play_count, playback_position, media_key);

-- Composite cover for episode queries that filter by both kind and grandparent_id.
-- Used by the NOT EXISTS unplayed-count query and per-series episode loads in nextup.
CREATE INDEX IF NOT EXISTS idx_media_kind_grandparent
  ON media(kind, grandparent_id);
