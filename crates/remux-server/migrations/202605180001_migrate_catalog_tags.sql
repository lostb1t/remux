-- Move any remaining catalog membership tags from media_tags into media_catalog_items.
-- Safe to run even if 202605170003 already migrated some rows: OR IGNORE skips duplicates.
-- Tag format: "catalog:{36-char-uuid}:{local_id}"
--   chars 1-8  = "catalog:"
--   chars 9-44 = addon UUID (36 chars)
--   char  45   = ":"
--   chars 46+  = local catalog ID
INSERT OR IGNORE INTO media_catalog_items (media_id, addon_id, catalog_id)
SELECT
    mt.media_id,
    substr(mt.tag, 9, 36),
    substr(mt.tag, 46)
FROM media_tags mt
WHERE mt.tag LIKE 'catalog:%';

DELETE FROM media_tags WHERE tag LIKE 'catalog:%';
