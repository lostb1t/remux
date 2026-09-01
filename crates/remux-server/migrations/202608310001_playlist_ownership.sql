ALTER TABLE media ADD COLUMN user_id TEXT;
ALTER TABLE media ADD COLUMN public INTEGER NOT NULL DEFAULT 0;

-- Assign existing playlists to the first admin user.
-- If no admin exists, mark them public so they remain accessible.
UPDATE media
SET user_id = (SELECT id FROM users WHERE is_admin = 1 ORDER BY rowid LIMIT 1),
    public   = CASE
                 WHEN (SELECT COUNT(*) FROM users WHERE is_admin = 1) = 0 THEN 1
                 ELSE 0
               END
WHERE kind = 'playlist'
  AND user_id IS NULL;
