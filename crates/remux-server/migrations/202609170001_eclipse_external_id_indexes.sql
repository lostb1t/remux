-- Mirrors 202609030003_media_music_external_id_indexes.sql for the two
-- identity fields Eclipse addons introduced: without these, every dedup/
-- identity lookup on an Eclipse-sourced item (`Media::find_by_external_ids`)
-- falls back to a full table scan on `media`.

CREATE INDEX IF NOT EXISTS idx_media_ext_eclipse_id
    ON media(json_extract(external_ids, '$.eclipse_id'))
    WHERE json_extract(external_ids, '$.eclipse_id') IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_media_ext_isrc
    ON media(json_extract(external_ids, '$.isrc'))
    WHERE json_extract(external_ids, '$.isrc') IS NOT NULL;
