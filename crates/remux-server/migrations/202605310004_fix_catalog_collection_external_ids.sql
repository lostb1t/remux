-- Fix catalog collection rows inserted without external_ids (NULL or empty string
-- causes sqlx json decoding to fail with "EOF while parsing a value").
UPDATE media SET external_ids = '{}' WHERE kind = 'collection' AND (external_ids IS NULL OR external_ids = '');
