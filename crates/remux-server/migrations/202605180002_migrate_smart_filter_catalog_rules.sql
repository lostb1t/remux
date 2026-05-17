-- Migrate smart collection filter rules that used FilterRule::Tag with catalog: values
-- to FilterRule::Collection.
--
-- FilterRule is serialized with serde(tag = "field"), so Tag → {"field":"tag",...}
-- and Collection → {"field":"collection",...}.
--
-- Two replacements per row:
--   1. Strip the "catalog:" prefix from each catalog tag value so
--      "catalog:{uuid}:{local}" → "{uuid}:{local}" (the format media_catalog_items stores).
--   2. Rename the field discriminant from "tag" to "collection".
--
-- Only rows whose smart filter JSON contains a catalog: value are touched.
-- The OR IGNORE on the INSERT in 202605180001 makes both migrations safe to re-run.
UPDATE media
SET collection_smart_filter = replace(
    replace(collection_smart_filter, '"catalog:', '"'),
    '"field":"tag"', '"field":"collection"'
)
WHERE collection_smart_filter LIKE '%"catalog:%';
