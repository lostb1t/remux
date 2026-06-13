-- Rename collection_id → catalog_id in stored catalog filter rules.
UPDATE media
SET collection_smart_filter = replace(
    collection_smart_filter,
    '"collection_id"',
    '"catalog_id"'
)
WHERE collection_smart_filter IS NOT NULL
  AND collection_smart_filter LIKE '%"field":"catalog"%';
