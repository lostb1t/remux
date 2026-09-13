-- Per-collection opt-out from the jellyfin-web home "My Media" section.
-- Distinct from `promoted`: a collection with promoted = 1 and
-- show_in_my_media = 0 keeps its sidebar entry, its "Latest" shelf and its
-- browse views, and is only skipped when the home My Media tiles are built.
ALTER TABLE media ADD COLUMN collection_show_in_my_media BOOLEAN NOT NULL DEFAULT 1;
