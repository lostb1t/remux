-- SQLite errors while building/maintaining a `json_extract` expression index
-- if any row's value isn't well-formed JSON. `external_ids` is written by
-- serde_json today, but `widen_external_ids`'s json_valid fallback exists
-- because malformed rows have been observed in the wild — repair those to
-- '{}' first so both this migration and future writes to the column can't
-- fail with "malformed JSON".
UPDATE media SET external_ids = '{}'
    WHERE external_ids IS NOT NULL AND NOT json_valid(external_ids);

CREATE INDEX IF NOT EXISTS idx_media_ext_imdb
    ON media(json_extract(external_ids, '$.imdb'))
    WHERE json_extract(external_ids, '$.imdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_tmdb
    ON media(json_extract(external_ids, '$.tmdb'))
    WHERE json_extract(external_ids, '$.tmdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_tvdb
    ON media(json_extract(external_ids, '$.tvdb'))
    WHERE json_extract(external_ids, '$.tvdb') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_kitsu
    ON media(json_extract(external_ids, '$.kitsu'))
    WHERE json_extract(external_ids, '$.kitsu') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_stremio_id
    ON media(json_extract(external_ids, '$.custom_stremio_id'))
    WHERE json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL;
