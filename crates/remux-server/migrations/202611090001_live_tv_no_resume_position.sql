-- Live TV has no position to resume from (Jellyfin stores 0 for it). Older
-- progress reports saved one for channels, which made clients open a channel
-- at that offset.
UPDATE user_media_state SET playback_position = 0
WHERE playback_position != 0
  AND media_id IN (SELECT id FROM media WHERE kind IN ('tv_channel', 'tv_program'));
