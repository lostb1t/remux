-- UUIDs stored as 16-byte blobs to match sqlx 0.8 Uuid encoding
INSERT OR IGNORE INTO media (id, title, kind, catalog_kind, catalog_media_kind, promoted, created_at, updated_at)
VALUES
    (x'a1b2c3d4000040008000000000000001', 'Movies', 'catalog', 'smart', 'movie',  1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP),
    (x'a1b2c3d4000040008000000000000002', 'Series', 'catalog', 'smart', 'series', 1, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
