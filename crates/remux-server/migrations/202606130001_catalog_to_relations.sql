-- Migrate catalog membership from media_catalog_items into media_relations (role='catalog').
-- The collection UUID is the id of the catalog collection whose collection_source matches
-- "addon_id:catalog_id".  Both are already in the media table from the previous migration.

INSERT OR IGNORE INTO media_relations (relation_id, left_media_id, right_media_id, role, weight)
SELECT
    -- deterministic relation_id: not strictly required but avoids a random BLOB per row
    lower(substr(hex(mci.media_id),1,8) ||'-'|| substr(hex(mci.media_id),9,4)  ||'-4'||
          substr(hex(mci.media_id),14,3)||'-'|| substr(hex(mci.media_id),17,4) ||'-'|| substr(hex(mci.media_id),21,12)),
    m.id,
    mci.media_id,
    'catalog',
    COALESCE(mci.item_order, 0)
FROM media_catalog_items mci
JOIN media m
  ON m.collection_source = mci.addon_id || ':' || mci.catalog_id
 AND m.collection_kind   = 'catalog';

-- Update FilterRule::Catalog in stored smart filters from the old
--   {"field":"catalog","addon_id":"...","catalog_id":"..."}
-- to the new
--   {"field":"catalog","collection_id":"<this collection's UUID>"}
-- Only catalog collections carry this rule, and it is always at rules[0].
UPDATE media
SET collection_smart_filter = json_set(
    collection_smart_filter,
    '$.rules[0]',
    json_object(
        'field', 'catalog',
        'collection_id',
        lower(
            substr(hex(id), 1, 8)  ||'-'||
            substr(hex(id), 9, 4)  ||'-'||
            substr(hex(id),13, 4)  ||'-'||
            substr(hex(id),17, 4)  ||'-'||
            substr(hex(id),21,12)
        )
    )
)
WHERE collection_kind = 'catalog'
  AND json_extract(collection_smart_filter, '$.rules[0].field') = 'catalog';

DROP TABLE media_catalog_items;
