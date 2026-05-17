CREATE TABLE media_catalog_items (
    media_id   TEXT NOT NULL REFERENCES media(id) ON DELETE CASCADE,
    addon_id   TEXT NOT NULL,
    catalog_id TEXT NOT NULL,
    PRIMARY KEY (media_id, addon_id, catalog_id)
);
CREATE INDEX idx_media_catalog_items_addon ON media_catalog_items(addon_id, catalog_id);

-- Migrate existing catalog tags. Format: "catalog:{36-char-uuid}:{local_id}"
-- chars 1-8 = "catalog:", UUID at chars 9..44, colon at 45, local_id at 46+.
INSERT OR IGNORE INTO media_catalog_items (media_id, addon_id, catalog_id)
SELECT
    mt.media_id,
    substr(mt.tag, 9, 36),
    substr(mt.tag, 46)
FROM media_tags mt
WHERE mt.tag LIKE 'catalog:%';

DELETE FROM media_tags WHERE tag LIKE 'catalog:%';
