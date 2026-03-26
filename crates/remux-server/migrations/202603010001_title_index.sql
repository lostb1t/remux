-- COLLATE NOCASE index lets SQLite use a B-tree scan for:
--   title LIKE 'X%'          (NameStartsWith)
--   title >= 'X' COLLATE NOCASE  (NameStartsWithOrGreater)
--   title < 'X'  COLLATE NOCASE  (NameLessThan)
-- A plain binary index on title would NOT be used for COLLATE NOCASE comparisons.
CREATE INDEX IF NOT EXISTS idx_media_title ON media(title COLLATE NOCASE);
