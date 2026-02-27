PRAGMA foreign_keys = OFF;

-- Widen collection_kind CHECK to include 'catalog'
-- Add collection_max_items column (nullable INTEGER)
CREATE TABLE media_new (
    id TEXT PRIMARY KEY NOT NULL,
    title TEXT NOT NULL,
    kind TEXT NOT NULL CHECK (kind IN ('movie', 'series', 'season', 'episode', 'person', 'studio', 'genre', 'collection', 'source', 'folder', 'unknown')),
    imdb_id TEXT,
    aio_id TEXT,
    series_imdb_id TEXT,
    parent_id TEXT,
    idx INTEGER,
    parent_idx INTEGER,
    released_at TIMESTAMP,
    runtime INTEGER,
    rating_critic REAL,
    rating_audience REAL,
    certification TEXT,
    poster TEXT,
    logo TEXT,
    backdrop TEXT,
    description TEXT,
    trailers TEXT,
    url TEXT,
    probe_data TEXT,
    remote_data TEXT,
    promoted INTEGER NOT NULL DEFAULT 0,
    collection_kind TEXT CHECK (collection_kind IN ('manual', 'smart', 'catalog')),
    collection_media_kind TEXT CHECK (collection_media_kind IN ('movie', 'series')),
    collection_max_items INTEGER,
    created_at TIMESTAMP NOT NULL,
    updated_at TIMESTAMP NOT NULL,
    refreshed_at TIMESTAMP,
    FOREIGN KEY (parent_id) REFERENCES media(id) ON DELETE CASCADE
);

INSERT INTO media_new (
    id, title, kind, imdb_id, aio_id, series_imdb_id, parent_id,
    idx, parent_idx, released_at, runtime, rating_critic, rating_audience,
    certification, poster, logo, backdrop, description, trailers,
    url, probe_data, remote_data, promoted,
    collection_kind, collection_media_kind, collection_max_items,
    created_at, updated_at, refreshed_at
)
SELECT
    id, title, kind, imdb_id, aio_id, series_imdb_id, parent_id,
    idx, parent_idx, released_at, runtime, rating_critic, rating_audience,
    certification, poster, logo, backdrop, description, trailers,
    url, probe_data, remote_data, promoted,
    collection_kind, collection_media_kind, NULL,
    created_at, updated_at, refreshed_at
FROM media;

DROP INDEX IF EXISTS idx_media_kind;
DROP INDEX IF EXISTS idx_media_idx;
DROP INDEX IF EXISTS idx_media_parent_id;
DROP INDEX IF EXISTS uniq_meta;
DROP TABLE media;
ALTER TABLE media_new RENAME TO media;

CREATE INDEX idx_media_kind ON media(kind);
CREATE INDEX idx_media_idx ON media(idx);
CREATE INDEX idx_media_parent_id ON media(parent_id);
CREATE UNIQUE INDEX uniq_meta ON media (kind, aio_id)
    WHERE kind IN ('movie', 'series', 'season', 'episode');

PRAGMA foreign_keys = ON;
