-- Local music: Deezer IDs resolved at scan time (mirroring imdb_id) so catalog
-- rows carry a real provider identity for metadata enrichment and validation.
ALTER TABLE opendal_files ADD COLUMN deezer_track INTEGER;
ALTER TABLE opendal_files ADD COLUMN deezer_album INTEGER;
ALTER TABLE opendal_files ADD COLUMN deezer_artist INTEGER;
