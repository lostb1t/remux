ALTER TABLE addons ADD COLUMN probe_on_scan BOOLEAN NOT NULL DEFAULT 0;
ALTER TABLE opendal_files ADD COLUMN probe_data TEXT;
ALTER TABLE opendal_files ADD COLUMN probe_version TEXT;
