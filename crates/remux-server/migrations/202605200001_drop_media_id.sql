-- Drop the uniq_meta index (keyed on media_id) and the media_id column.
-- Stable UUID (id) is now the single identity mechanism.
-- A programmatic startup migration must run first to update existing UUIDs
-- and migrate user_media_state.media_key values to UUID format.
DROP INDEX IF EXISTS uniq_meta;
DROP INDEX IF EXISTS idx_media_media_id;
ALTER TABLE media DROP COLUMN media_id;
