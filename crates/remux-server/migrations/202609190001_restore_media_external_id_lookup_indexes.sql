-- 202609080002_media_external_id_unique_indexes.sql dropped the single-column
-- partial expression indexes (idx_media_ext_imdb, _tmdb, _tvdb, _kitsu,
-- _stremio_id) and replaced them with `(kind, json_extract(...))` unique
-- indexes to enforce one-external-id-per-kind at the DB level.
--
-- That unique constraint is real and still needed — but it isn't what
-- `Media::find_by_external_ids` queries against. That lookup is
-- `kind = ? AND (json_extract(...,'$.imdb') = ? OR json_extract(...,'$.tmdb')
-- = ? OR ...)`, and SQLite cannot prove the new composite indexes' partial
-- predicate applies to it — confirmed directly: `EXPLAIN QUERY PLAN` on that
-- exact query falls back to `SEARCH media USING INDEX
-- idx_media_kind_avail_sentinel (kind=?)` (a full scan of the whole `kind`
-- partition), and forcing `INDEXED BY idx_media_ext_imdb_unique` on it
-- returns a flat "no query solution" — SQLite genuinely cannot serve this
-- query shape from that index, not just declining to.
--
-- A unique index still enforces its constraint on every INSERT/UPDATE
-- regardless of whether the query planner ever picks it for a SELECT, so the
-- fix is purely additive: restore the original single-column lookup indexes
-- alongside the existing unique ones. The unique indexes keep the identity
-- guarantee; these give `find_by_external_ids` something it can actually use
-- again, restoring the same query plan (and cost) it had before that
-- migration.

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
