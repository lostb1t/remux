-- A check-then-insert race in metadata import let two rows for the same
-- real Movie/Series/TvProgram get created under different UUIDs when they
-- shared an external id (imdb/tmdb/tvdb/kitsu/custom_stremio_id) but arrived
-- concurrently, before either had committed for the other to find. This
-- collapses any such existing duplicates before the next migration adds
-- UNIQUE indexes that make the race impossible going forward — those
-- indexes cannot be created while duplicate data still exists.
--
-- One pass covers all five fields (a loser can match its winner via more
-- than one field at once — that's the common case in practice — and
-- INSERT OR IGNORE just keeps the first mapping found for it). Each of the
-- five per-field lookups already has a matching partial index from an
-- earlier migration (idx_media_ext_imdb/tmdb/tvdb/kitsu/stremio_id), so
-- finding duplicate groups is an index scan, not a table scan; the actual
-- remap/delete steps below are driven by however few duplicate rows this
-- finds (a handful, not a fraction of the table), using the existing
-- indexes on parent_id/grandparent_id/media_relations/user_media_state to
-- locate what references them. If some pathological multi-field chain
-- isn't fully collapsed by this single pass, the next migration's
-- CREATE UNIQUE INDEX fails loudly rather than silently losing data.

CREATE TEMP TABLE _dedupe_map (loser_id TEXT PRIMARY KEY, winner_id TEXT NOT NULL);

INSERT OR IGNORE INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.field, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, g.field) = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, '$.imdb' AS field, json_extract(external_ids, '$.imdb') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.imdb') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
        UNION ALL
        SELECT kind, '$.custom_stremio_id', json_extract(external_ids, '$.custom_stremio_id')
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL
        GROUP BY kind, json_extract(external_ids, '$.custom_stremio_id') HAVING count(*) > 1
        UNION ALL
        SELECT kind, '$.tmdb', json_extract(external_ids, '$.tmdb')
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.tmdb') IS NOT NULL
        GROUP BY kind, json_extract(external_ids, '$.tmdb') HAVING count(*) > 1
        UNION ALL
        SELECT kind, '$.tvdb', json_extract(external_ids, '$.tvdb')
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.tvdb') IS NOT NULL
        GROUP BY kind, json_extract(external_ids, '$.tvdb') HAVING count(*) > 1
        UNION ALL
        SELECT kind, '$.kitsu', json_extract(external_ids, '$.kitsu')
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.kitsu') IS NOT NULL
        GROUP BY kind, json_extract(external_ids, '$.kitsu') HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, w.field) = w.val
WHERE m.id != w.winner_id;

-- Repoint season/episode trees before deleting losers — parent_id cascades
-- on delete, so skipping this would silently wipe a loser series' children.
UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

-- Merge watch state/favorites instead of blindly overwriting — a loser row
-- may already have real user activity attached.
INSERT INTO user_media_state (user_id, media_id, media_raw, favorite, play_count, played_at, playback_position, stream_id, subtitle_idx, audio_idx, last_played_at, rating)
SELECT user_id, (SELECT winner_id FROM _dedupe_map WHERE loser_id = user_media_state.media_id),
       media_raw, favorite, play_count, played_at, playback_position, stream_id, subtitle_idx, audio_idx, last_played_at, rating
FROM user_media_state WHERE media_id IN (SELECT loser_id FROM _dedupe_map)
ON CONFLICT(user_id, media_id) DO UPDATE SET
    favorite = MAX(user_media_state.favorite, excluded.favorite),
    play_count = user_media_state.play_count + excluded.play_count,
    played_at = MAX(user_media_state.played_at, excluded.played_at),
    last_played_at = MAX(user_media_state.last_played_at, excluded.last_played_at),
    rating = COALESCE(user_media_state.rating, excluded.rating);
DELETE FROM user_media_state WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

-- Repoint relations. Dropping only rows that collide with an existing
-- winner-owned relation isn't enough: two *sibling* losers merging into the
-- same winner can each hold an identical (right_media_id, role) relation,
-- which would only collide with each other once both are remapped in the
-- same UPDATE below — a pre-remap EXISTS check against the current table
-- can't see that. Instead, group every relation that will end up owned by a
-- given winner (its own pre-existing ones plus every loser's) by
-- (target left id, right_media_id, role) and keep exactly one per group —
-- preferring a row the winner already owns, else the lowest relation_id —
-- before remapping survivors.
DELETE FROM media_relations WHERE relation_id IN (
    SELECT relation_id FROM (
        SELECT mr.relation_id,
            ROW_NUMBER() OVER (
                PARTITION BY COALESCE(dm.winner_id, mr.left_media_id), mr.right_media_id, COALESCE(mr.role, '')
                ORDER BY (dm.winner_id IS NULL) DESC, mr.relation_id ASC
            ) AS rn
        FROM media_relations mr
        LEFT JOIN _dedupe_map dm ON dm.loser_id = mr.left_media_id
        WHERE dm.winner_id IS NOT NULL OR mr.left_media_id IN (SELECT winner_id FROM _dedupe_map)
    ) WHERE rn > 1
);
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE relation_id IN (
    SELECT relation_id FROM (
        SELECT mr.relation_id,
            ROW_NUMBER() OVER (
                PARTITION BY mr.left_media_id, COALESCE(dm.winner_id, mr.right_media_id), COALESCE(mr.role, '')
                ORDER BY (dm.winner_id IS NULL) DESC, mr.relation_id ASC
            ) AS rn
        FROM media_relations mr
        LEFT JOIN _dedupe_map dm ON dm.loser_id = mr.right_media_id
        WHERE dm.winner_id IS NOT NULL OR mr.right_media_id IN (SELECT winner_id FROM _dedupe_map)
    ) WHERE rn > 1
);
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

-- Popularity is a recomputed rollup; just drop the loser's rows.
DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

-- media_tags/media_images cascade-delete with the row below (ON DELETE
-- CASCADE); the surviving winner regains them on its next metadata refresh.
DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DROP TABLE _dedupe_map;
