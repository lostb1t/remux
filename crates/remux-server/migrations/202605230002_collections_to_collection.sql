-- Upgrade existing hardcoded "Collections" Folder to a proper Collection
UPDATE media
SET kind = 'collection',
    collection_kind = 'manual',
    collection_media_kind = 'collection',
    promoted = 1
WHERE id = x'f47ac10b58cc4372a5670e02b2c3d479';

-- Fresh install: insert if not present
INSERT OR IGNORE INTO media (id, title, kind, collection_kind, collection_media_kind, promoted, created_at, updated_at)
VALUES (
    x'f47ac10b58cc4372a5670e02b2c3d479',
    'Collections',
    'collection',
    'manual',
    'collection',
    1,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP
);
