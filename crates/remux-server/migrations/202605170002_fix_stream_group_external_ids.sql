UPDATE media SET external_ids = '{}' WHERE kind = 'stream_group' AND external_ids IS NULL;
