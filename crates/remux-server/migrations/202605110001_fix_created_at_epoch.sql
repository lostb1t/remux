-- Fix media rows where created_at was never set (defaulted to Unix epoch).
-- Set them to updated_at which is always maintained, so ordering is at least meaningful.
UPDATE media
SET created_at = updated_at
WHERE created_at = '1970-01-01 00:00:00';
