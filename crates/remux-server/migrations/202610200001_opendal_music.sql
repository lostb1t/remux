-- Local music: artist/album parsed from the Jellyfin folder layout
-- ({Artist}/{Album}/{Track}, album-only folders, loose tracks) at scan time.
ALTER TABLE opendal_files ADD COLUMN artist TEXT;
ALTER TABLE opendal_files ADD COLUMN album TEXT;
CREATE INDEX IF NOT EXISTS idx_opendal_files_artist_album
    ON opendal_files(addon_id, media_kind, artist, album);
