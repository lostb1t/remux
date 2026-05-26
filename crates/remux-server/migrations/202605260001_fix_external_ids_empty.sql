-- Fix rows where external_ids is an empty string (causes JSON decode failure).
-- Set to '{}' (empty object) so it always deserializes cleanly.
UPDATE media SET external_ids = '{}' WHERE external_ids = '' OR external_ids IS NULL;
