-- Enforce at the DB level what `Media::find_by_external_ids` already treats
-- as identity: within a given kind, a single external id can only ever
-- belong to one row. Without this, concurrent metadata imports can each
-- check "does this external id already exist?", both get "no" because
-- neither has committed yet, and both insert — see the migration before
-- this one, which cleans up existing duplicates so these can be created.
--
-- Scoped by (kind, value): tmdb/tvdb ids are separate numbering spaces for
-- movies vs. tv, so the same numeric id legitimately appears once per kind.
-- This mirrors `find_by_external_ids`'s own `WHERE kind = ?` scoping exactly.

DROP INDEX IF EXISTS idx_media_ext_imdb;
DROP INDEX IF EXISTS idx_media_ext_tmdb;
DROP INDEX IF EXISTS idx_media_ext_tvdb;
DROP INDEX IF EXISTS idx_media_ext_kitsu;
DROP INDEX IF EXISTS idx_media_ext_stremio_id;

CREATE UNIQUE INDEX idx_media_ext_imdb_unique
    ON media(kind, json_extract(external_ids, '$.imdb'))
    WHERE json_extract(external_ids, '$.imdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_tmdb_unique
    ON media(kind, json_extract(external_ids, '$.tmdb'))
    WHERE json_extract(external_ids, '$.tmdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_tvdb_unique
    ON media(kind, json_extract(external_ids, '$.tvdb'))
    WHERE json_extract(external_ids, '$.tvdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_kitsu_unique
    ON media(kind, json_extract(external_ids, '$.kitsu'))
    WHERE json_extract(external_ids, '$.kitsu') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_stremio_id_unique
    ON media(kind, json_extract(external_ids, '$.custom_stremio_id'))
    WHERE json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL;
