-- Clear stale parent_id on collections that were manually assigned to a group container.
UPDATE media
SET parent_id = NULL
WHERE parent_id IN (
    SELECT id FROM media
    WHERE collection_kind = 'manual'
      AND collection_media_kind = 'collection'
);

-- Convert all manual group containers to smart. Empty filter = show all unclaimed collections.
UPDATE media
SET collection_kind = 'smart'
WHERE collection_kind = 'manual'
  AND collection_media_kind = 'collection';
