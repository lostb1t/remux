-- Collections had their sort_order written into idx by mistake in the
-- create/update virtual folder handlers. Copy idx -> sort_order where
-- sort_order is not already set, then clear idx on all collections.
UPDATE media
SET sort_order = idx
WHERE kind = 'collection'
  AND idx IS NOT NULL
  AND sort_order IS NULL;

UPDATE media
SET idx = NULL
WHERE kind = 'collection';
