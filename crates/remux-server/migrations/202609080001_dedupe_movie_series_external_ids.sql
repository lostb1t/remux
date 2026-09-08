-- A check-then-insert race in metadata import let two rows for the same
-- real Movie/Series/TvProgram get created under different UUIDs when they
-- shared an external id (imdb/tmdb/tvdb/kitsu/custom_stremio_id) but arrived
-- concurrently, before either had committed for the other to find. This
-- collapses any such existing duplicates before the next migration adds
-- UNIQUE indexes that make the race impossible going forward — those
-- indexes cannot be created while duplicate data still exists.
--
-- Run once per external-id field. Each pass recomputes duplicate groups
-- against the table's current (post-previous-pass) state, so a row already
-- merged by an earlier field's pass is simply absent from later passes —
-- this also correctly collapses chains where a row duplicates one row via
-- one field and a different row via another. If some pathological chain
-- still isn't fully collapsed after all five passes, the next migration's
-- CREATE UNIQUE INDEX will fail loudly rather than silently losing data.

DROP TABLE IF EXISTS temp._dedupe_map;
CREATE TEMP TABLE _dedupe_map (loser_id TEXT PRIMARY KEY, winner_id TEXT NOT NULL);

-- imdb
INSERT INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, '$.imdb') = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, json_extract(external_ids, '$.imdb') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.imdb') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, '$.imdb') = w.val
WHERE m.id != w.winner_id;

UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

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

DELETE FROM media_relations WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.left_media_id)
      AND mr2.right_media_id = media_relations.right_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.right_media_id)
      AND mr2.left_media_id = media_relations.left_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

-- media_tags/media_images cascade-delete with the row below (ON DELETE
-- CASCADE); the surviving winner regains them on its next metadata refresh.
DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM _dedupe_map;

-- tmdb
INSERT INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, '$.tmdb') = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, json_extract(external_ids, '$.tmdb') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.tmdb') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, '$.tmdb') = w.val
WHERE m.id != w.winner_id;

UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

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

DELETE FROM media_relations WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.left_media_id)
      AND mr2.right_media_id = media_relations.right_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.right_media_id)
      AND mr2.left_media_id = media_relations.left_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM _dedupe_map;

-- tvdb
INSERT INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, '$.tvdb') = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, json_extract(external_ids, '$.tvdb') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.tvdb') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, '$.tvdb') = w.val
WHERE m.id != w.winner_id;

UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

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

DELETE FROM media_relations WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.left_media_id)
      AND mr2.right_media_id = media_relations.right_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.right_media_id)
      AND mr2.left_media_id = media_relations.left_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM _dedupe_map;

-- kitsu
INSERT INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, '$.kitsu') = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, json_extract(external_ids, '$.kitsu') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.kitsu') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, '$.kitsu') = w.val
WHERE m.id != w.winner_id;

UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

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

DELETE FROM media_relations WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.left_media_id)
      AND mr2.right_media_id = media_relations.right_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.right_media_id)
      AND mr2.left_media_id = media_relations.left_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM _dedupe_map;

-- custom_stremio_id
INSERT INTO _dedupe_map (loser_id, winner_id)
SELECT m.id, w.winner_id
FROM media m
JOIN (
    SELECT g.kind, g.val,
        (SELECT id FROM media m2
         WHERE m2.kind = g.kind AND json_extract(m2.external_ids, '$.custom_stremio_id') = g.val
         ORDER BY m2.created_at ASC, m2.id ASC LIMIT 1) AS winner_id
    FROM (
        SELECT kind, json_extract(external_ids, '$.custom_stremio_id') AS val
        FROM media
        WHERE kind IN ('movie', 'series', 'tv_program')
          AND json_extract(external_ids, '$.custom_stremio_id') IS NOT NULL
        GROUP BY kind, val HAVING count(*) > 1
    ) g
) w ON m.kind = w.kind AND json_extract(m.external_ids, '$.custom_stremio_id') = w.val
WHERE m.id != w.winner_id;

UPDATE media SET parent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.parent_id)
WHERE parent_id IN (SELECT loser_id FROM _dedupe_map);
UPDATE media SET grandparent_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media.grandparent_id)
WHERE grandparent_id IN (SELECT loser_id FROM _dedupe_map);

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

DELETE FROM media_relations WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.left_media_id)
      AND mr2.right_media_id = media_relations.right_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET left_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = left_media_id)
WHERE left_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media_relations WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map)
  AND EXISTS (SELECT 1 FROM media_relations mr2
    WHERE mr2.right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = media_relations.right_media_id)
      AND mr2.left_media_id = media_relations.left_media_id
      AND COALESCE(mr2.role, '') = COALESCE(media_relations.role, ''));
UPDATE media_relations SET right_media_id = (SELECT winner_id FROM _dedupe_map WHERE loser_id = right_media_id)
WHERE right_media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM popularity_raw WHERE media_id IN (SELECT loser_id FROM _dedupe_map);
DELETE FROM popularity_agg WHERE media_id IN (SELECT loser_id FROM _dedupe_map);

DELETE FROM media WHERE id IN (SELECT loser_id FROM _dedupe_map);

DROP TABLE _dedupe_map;
