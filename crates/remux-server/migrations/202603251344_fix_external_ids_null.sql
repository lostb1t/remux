-- Ensure external_ids is never NULL — default to empty JSON object.
-- Rows inserted before this migration (or with no imdb_id) may have NULL.
UPDATE media SET external_ids = '{}' WHERE external_ids IS NULL OR external_ids = '';
