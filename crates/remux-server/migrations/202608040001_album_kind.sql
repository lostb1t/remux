-- Deezer record type for Album rows ("album" | "single" | "ep"); NULL = unknown.
-- Keeps singles/EPs out of the Albums section (filtered at query time).
ALTER TABLE media ADD COLUMN album_kind TEXT;
