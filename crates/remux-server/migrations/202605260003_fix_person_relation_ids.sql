-- Fix-up for migration 202605260002: the relation_id values it inserted were
-- TEXT (hyphenated UUID strings), but SQLx stores Uuid as a 16-byte BLOB.
-- Reading those TEXT rows causes a ParseByteLength panic at runtime.
--
-- Step 1: delete the TEXT relation_id rows (safe — TMDB will re-link on next enrichment).
-- Step 2: re-run the re-pointing INSERT with randomblob(16) so any remaining
--         name-keyed-to-tmdb remaps are stored as proper BLOBs.
-- Step 3: delete remaining relations pointing at name-keyed persons (same as 202605260002
--         step 2, idempotent if already done).
-- Step 4: delete orphaned name-keyed person rows (same as 202605260002 step 3, idempotent).

-- Step 1: remove TEXT-format relation_id rows left by 202605260002.
DELETE FROM media_relations
WHERE typeof(relation_id) = 'text';

-- Step 2: re-insert the remapped relations with BLOB relation_ids.
INSERT OR IGNORE INTO media_relations
    (relation_id, left_media_id, right_media_id, weight, role, character)
SELECT
    randomblob(16),
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

-- Step 3: delete remaining relations pointing at name-keyed persons.
DELETE FROM media_relations
WHERE right_media_id IN (
    SELECT id FROM media
    WHERE kind = 'person'
      AND json_extract(external_ids, '$.tmdb') IS NULL
);

-- Step 4: delete orphaned name-keyed person rows.
DELETE FROM media
WHERE kind = 'person'
  AND json_extract(external_ids, '$.tmdb') IS NULL;
