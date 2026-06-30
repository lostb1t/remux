ALTER TABLE addons ADD COLUMN system INTEGER NOT NULL DEFAULT 0;

-- Mark the default TMDB addon as a system addon (cannot be deleted or modified by users).
UPDATE addons SET system = 1 WHERE id = x'df84acaafe344fe7b82615599646062e';
