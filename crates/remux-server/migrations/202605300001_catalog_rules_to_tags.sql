-- Migrate FilterRule::Collection → FilterRule::Tag in smart collection filters and user policies.
-- Tags are set to the full "addon_uuid:catalog_id" value so they match what the old rule matched.

-- Step A: tag media from catalog rules stored on smart collections.
INSERT OR IGNORE INTO media_tags (media_id, tag)
SELECT mci.media_id, r.value
FROM media m,
     json_each(json_extract(m.collection_smart_filter, '$.rules')) AS rules,
     json_each(rules.value, '$.values') AS r
JOIN media_catalog_items mci
  ON lower(mci.addon_id || ':' || mci.catalog_id) = lower(r.value)
WHERE m.kind = 'collection'
  AND m.collection_smart_filter IS NOT NULL
  AND json_extract(rules.value, '$.field') = 'collection';

-- Step B: tag media from catalog rules stored in user policy filter_rules.
INSERT OR IGNORE INTO media_tags (media_id, tag)
SELECT mci.media_id, r.value
FROM users u,
     json_each(json_extract(u.policy, '$.filter_rules.rules')) AS rules,
     json_each(rules.value, '$.values') AS r
JOIN media_catalog_items mci
  ON lower(mci.addon_id || ':' || mci.catalog_id) = lower(r.value)
WHERE u.policy IS NOT NULL
  AND json_extract(rules.value, '$.field') = 'collection';

-- Step C: rename field in collection smart filters.
UPDATE media
SET collection_smart_filter = replace(
    collection_smart_filter, '"field":"collection"', '"field":"tag"'
)
WHERE collection_smart_filter LIKE '%"field":"collection"%';

-- Step D: rename field in user policy filter_rules.
UPDATE users
SET policy = replace(
    policy, '"field":"collection"', '"field":"tag"'
)
WHERE policy LIKE '%"field":"collection"%';
