ALTER TABLE stream_groups ADD COLUMN filter TEXT;
ALTER TABLE stream_groups DROP COLUMN resolution;
ALTER TABLE stream_groups DROP COLUMN quality;
