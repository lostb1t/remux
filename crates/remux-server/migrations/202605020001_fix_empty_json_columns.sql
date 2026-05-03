-- Some rows have '' (empty string) instead of NULL in JSON columns, which
-- causes sqlx's json decoder to fail with "EOF while parsing a value".
UPDATE media SET trailers               = NULL WHERE trailers               = '';
UPDATE media SET probe_data             = NULL WHERE probe_data             = '';
UPDATE media SET provider_info          = NULL WHERE provider_info          = '';
UPDATE media SET collection_smart_filter = NULL WHERE collection_smart_filter = '';
