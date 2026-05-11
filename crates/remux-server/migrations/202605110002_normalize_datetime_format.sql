-- Normalize created_at and updated_at to consistent ISO 8601 format (YYYY-MM-DD HH:MM:SS.SSS)
-- so that text-based ORDER BY works correctly without needing datetime() wrapping.
UPDATE media
SET created_at = strftime('%Y-%m-%dT%H:%M:%f', created_at)
WHERE created_at IS NOT NULL;

UPDATE media
SET updated_at = strftime('%Y-%m-%dT%H:%M:%f', updated_at)
WHERE updated_at IS NOT NULL;
