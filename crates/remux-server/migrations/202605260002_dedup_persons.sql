-- Deduplicate person rows: make TMDB ID the sole canonical key for persons.
--
-- Background: Stremio/Jellyfin addons created person rows keyed by name
-- (stable_media_uuid(Person, name_lowercase)), while the TMDB addon creates
-- person rows keyed by TMDB ID (stable_media_uuid(Person, tmdb_id)).
-- This left duplicate rows for the same person.
--
-- Strategy:
--   1. Re-point media_relations from name-keyed persons to their unique TMDB-keyed twin.
--   2. Delete all remaining relations pointing to name-keyed persons.
--   3. Delete all name-keyed person rows (persons without a TMDB ID).
--
-- After this migration, persons without a TMDB ID simply don't exist in the DB.
-- The TMDB addon will recreate them with correct IDs when it next enriches
-- the parent movie/series.

-- Step 1: Re-point relations from name-keyed persons to their unique TMDB-keyed twin
-- (only where exactly one TMDB-keyed person matches by title — avoids merging
-- distinct people who share a display name).
INSERT OR IGNORE INTO media_relations
    (relation_id, left_media_id, right_media_id, weight, role, character)
SELECT
    lower(hex(randomblob(4)) || '-' || hex(randomblob(2)) || '-4' ||
          substr(hex(randomblob(2)),2) || '-' ||
          substr('89ab', abs(random()) % 4 + 1, 1) || substr(hex(randomblob(2)),2) || '-' ||
          hex(randomblob(6))),
    mr.left_media_id,
    tmdb_p.id,
    mr.weight,
    mr.role,
    mr.character
FROM media_relations mr
JOIN media name_p ON name_p.id = mr.right_media_id
    AND name_p.kind = 'person'
    AND json_extract(name_p.external_ids, '$.tmdb') IS NULL
JOIN media tmdb_p ON tmdb_p.kind = 'person'
    AND lower(tmdb_p.title) = lower(name_p.title)
    AND json_extract(tmdb_p.external_ids, '$.tmdb') IS NOT NULL
WHERE (
    SELECT count(*) FROM media c
    WHERE c.kind = 'person'
      AND lower(c.title) = lower(name_p.title)
      AND json_extract(c.external_ids, '$.tmdb') IS NOT NULL
) = 1;

-- Step 2: Delete all relations still pointing to name-keyed persons.
-- (Covers both those that were re-pointed above and those with no TMDB twin.)
DELETE FROM media_relations
WHERE right_media_id IN (
    SELECT id FROM media
    WHERE kind = 'person'
      AND json_extract(external_ids, '$.tmdb') IS NULL
);

-- Step 3: Delete all name-keyed person rows.
-- Relations were cleaned in step 2, so no FK violations.
DELETE FROM media
WHERE kind = 'person'
  AND json_extract(external_ids, '$.tmdb') IS NULL;
