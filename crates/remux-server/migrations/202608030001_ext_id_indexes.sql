-- Expression indexes for external ID deduplication at upsert time.
-- Allows find_existing_id_by_ext() to run O(log n) lookups per root item
-- instead of a full table scan.

-- Root-level: Movie / Series
CREATE INDEX IF NOT EXISTS idx_media_ext_imdb
    ON media(kind, json_extract(external_ids, '$.imdb'))
    WHERE json_extract(external_ids, '$.imdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_tmdb
    ON media(kind, json_extract(external_ids, '$.tmdb'))
    WHERE json_extract(external_ids, '$.tmdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_tvdb
    ON media(kind, json_extract(external_ids, '$.tvdb'))
    WHERE json_extract(external_ids, '$.tvdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_kitsu
    ON media(kind, json_extract(external_ids, '$.kitsu'))
    WHERE json_extract(external_ids, '$.kitsu') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_custom_stremio_id
    ON media(kind, json_extract(external_ids, '$.custom_stremio_id'))
    WHERE json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL;

-- Music root deduplication
CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_artist
    ON media(json_extract(external_ids, '$.deezer_artist'))
    WHERE json_extract(external_ids, '$.deezer_artist') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_album
    ON media(json_extract(external_ids, '$.deezer_album'))
    WHERE json_extract(external_ids, '$.deezer_album') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_track
    ON media(json_extract(external_ids, '$.deezer_track'))
    WHERE json_extract(external_ids, '$.deezer_track') IS NOT NULL;

-- Child deduplication: Season / Episode / Track by (parent_id, kind, idx).
-- SQLite does not auto-index FK columns.
CREATE INDEX IF NOT EXISTS idx_media_parent_kind_idx
    ON media(parent_id, kind, idx)
    WHERE parent_id IS NOT NULL;
