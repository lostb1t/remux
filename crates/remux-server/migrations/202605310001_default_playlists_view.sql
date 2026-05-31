-- Add a default promoted Playlists container to user views (mirrors the Collections row).
INSERT OR IGNORE INTO media (id, title, kind, collection_kind, collection_media_kind, promoted, created_at, updated_at)
VALUES (
    x'211d58cdf8504c74b4866f15480ebcc3',
    'Playlists',
    'collection',
    'smart',
    'playlist',
    1,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP
);
