DROP INDEX IF EXISTS uniq_meta;
CREATE UNIQUE INDEX uniq_meta ON media (kind, media_id)
    WHERE media_id IS NOT NULL AND kind != 'tv_channel';
