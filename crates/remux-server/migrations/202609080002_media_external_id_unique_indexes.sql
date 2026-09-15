-- Enforce at the DB level what `Media::find_by_external_ids` already treats
-- as identity: within a given kind, a single external id can only ever
-- belong to one row. Without this, concurrent metadata imports can each
-- check "does this external id already exist?", both get "no" because
-- neither has committed yet, and both insert — see the migration before
-- this one, which cleans up existing duplicates so these can be created.
--
-- Scoped to Movie/Series/TvProgram only, matching `find_by_external_ids`'s
-- own match arm exactly (season/episode identity is positional — parent_id
-- + idx — never external-id based, and Person/Artist/Album/Track use their
-- own separate id schemes). Without this kind restriction, an Episode's own
-- per-episode tmdb id gets swept into the same constraint — and duplicate
-- episode trees (a known, separate consequence of two duplicate Series rows
-- each having their own full season/episode tree before being merged) then
-- violate it, blocking migration on any install that has that.

DROP INDEX IF EXISTS idx_media_ext_imdb;
DROP INDEX IF EXISTS idx_media_ext_tmdb;
DROP INDEX IF EXISTS idx_media_ext_tvdb;
DROP INDEX IF EXISTS idx_media_ext_kitsu;
DROP INDEX IF EXISTS idx_media_ext_stremio_id;

CREATE UNIQUE INDEX idx_media_ext_imdb_unique
    ON media(kind, json_extract(external_ids, '$.imdb'))
    WHERE kind IN ('movie', 'series', 'tv_program')
      AND json_extract(external_ids, '$.imdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_tmdb_unique
    ON media(kind, json_extract(external_ids, '$.tmdb'))
    WHERE kind IN ('movie', 'series', 'tv_program')
      AND json_extract(external_ids, '$.tmdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_tvdb_unique
    ON media(kind, json_extract(external_ids, '$.tvdb'))
    WHERE kind IN ('movie', 'series', 'tv_program')
      AND json_extract(external_ids, '$.tvdb') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_kitsu_unique
    ON media(kind, json_extract(external_ids, '$.kitsu'))
    WHERE kind IN ('movie', 'series', 'tv_program')
      AND json_extract(external_ids, '$.kitsu') IS NOT NULL;

CREATE UNIQUE INDEX idx_media_ext_stremio_id_unique
    ON media(kind, json_extract(external_ids, '$.custom_stremio_id'))
    WHERE kind IN ('movie', 'series', 'tv_program')
      AND json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL;
