INSERT INTO media (id, title, kind, collection_kind, collection_media_kind, external_ids, promoted, created_at, updated_at)
SELECT x'a1b2c3d4000040008000000000000003', 'Music', 'collection', 'smart', 'music', '{}', 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP
WHERE NOT EXISTS (SELECT 1 FROM media WHERE kind = 'collection' AND collection_media_kind = 'music');
