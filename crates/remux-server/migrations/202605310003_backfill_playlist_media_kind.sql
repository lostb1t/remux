-- Backfill collection_media_kind on existing playlists so MediaType is "Audio"/"Video"
-- instead of "Unknown". Uses the same kind-detection logic as sync_playlist_media_kind().
UPDATE media
SET collection_media_kind = CASE
    WHEN (
        SELECT m.kind FROM media_relations mr
        JOIN media m ON m.id = mr.right_media_id
        WHERE mr.left_media_id = media.id AND mr.role = 'playlist'
        ORDER BY mr.weight ASC LIMIT 1
    ) IN ('track', 'album', 'artist') THEN 'music'
    WHEN (
        SELECT m.kind FROM media_relations mr
        JOIN media m ON m.id = mr.right_media_id
        WHERE mr.left_media_id = media.id AND mr.role = 'playlist'
        ORDER BY mr.weight ASC LIMIT 1
    ) IS NOT NULL THEN 'movie'
    ELSE collection_media_kind
END
WHERE kind = 'playlist'
  AND collection_media_kind IS NULL;
