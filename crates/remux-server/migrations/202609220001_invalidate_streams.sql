-- Stream rows are disposable addon results. Remove them so refreshed rows use
-- the current ffprobe mapping, and expire their parents' stream refresh cache
-- so the addons are queried again immediately.
UPDATE media
SET streams_refreshed_at = NULL
WHERE id IN (
    SELECT DISTINCT parent_id
    FROM media
    WHERE kind = 'stream' AND parent_id IS NOT NULL
);

DELETE FROM media_relations
WHERE left_media_id IN (SELECT id FROM media WHERE kind = 'stream');

DELETE FROM media WHERE kind = 'stream';
