ALTER TABLE media ADD COLUMN user_id TEXT;
ALTER TABLE media ADD COLUMN public INTEGER NOT NULL DEFAULT 0;

-- Assign existing playlists to the first admin user.
UPDATE media
SET user_id = (SELECT id FROM users WHERE is_admin = 1 ORDER BY rowid LIMIT 1)
WHERE kind = 'playlist'
  AND user_id IS NULL;
