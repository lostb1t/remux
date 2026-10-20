-- Local music: artist/album parsed from the Jellyfin folder layout
-- ({Artist}/{Album}/{Track}, album-only folders, loose tracks) at scan time,
-- plus the disc number from a `Disc N` folder inside the album.
ALTER TABLE opendal_files ADD COLUMN artist TEXT;
ALTER TABLE opendal_files ADD COLUMN album TEXT;
ALTER TABLE opendal_files ADD COLUMN disc_number INTEGER;
CREATE INDEX IF NOT EXISTS idx_opendal_files_artist_album
    ON opendal_files(addon_id, media_kind, artist, album);
