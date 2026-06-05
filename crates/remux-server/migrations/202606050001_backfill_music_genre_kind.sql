-- Backfill genres linked to music content (track/album/artist) to music_genre kind.
UPDATE media
SET kind = 'music_genre'
WHERE kind = 'genre'
  AND id IN (
      SELECT DISTINCT mr.right_media_id
      FROM media_relations mr
      JOIN media m ON mr.left_media_id = m.id
      WHERE m.kind IN ('track', 'album', 'artist')
  );
