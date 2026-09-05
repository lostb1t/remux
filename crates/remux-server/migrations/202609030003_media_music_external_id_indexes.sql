CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_artist
    ON media(json_extract(external_ids, '$.deezer_artist'))
    WHERE json_extract(external_ids, '$.deezer_artist') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_album
    ON media(json_extract(external_ids, '$.deezer_album'))
    WHERE json_extract(external_ids, '$.deezer_album') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_deezer_track
    ON media(json_extract(external_ids, '$.deezer_track'))
    WHERE json_extract(external_ids, '$.deezer_track') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_youtube_id
    ON media(json_extract(external_ids, '$.youtube_id'))
    WHERE json_extract(external_ids, '$.youtube_id') IS NOT NULL;
