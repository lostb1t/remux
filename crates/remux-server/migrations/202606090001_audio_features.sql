-- Optional acoustic-similarity features for the music "instant mix" recommender.
--
-- One row per media item. Track rows hold the track's (Spotify-derived) audio
-- features; album and artist rows hold the precomputed centroid (mean) of their
-- tracks' features — so a mix seed's vector is a single indexed point lookup
-- rather than a per-request aggregation. All feature columns are normalized to
-- [0,1] at import time so the server can use a plain Euclidean distance and
-- never has to re-scale.
--
-- Populated offline by the dataset importer (see crates/remux-import-features);
-- empty by default, and the mix scorer only consults it when audio-feature
-- mixing is enabled in settings. Compact + WITHOUT ROWID for fast point reads.
CREATE TABLE IF NOT EXISTS media_features (
    media_id         BLOB PRIMARY KEY,
    danceability     REAL,
    energy           REAL,
    valence          REAL,
    tempo            REAL,
    acousticness     REAL,
    instrumentalness REAL,
    loudness         REAL,
    speechiness      REAL,
    liveness         REAL,
    popularity       REAL,
    FOREIGN KEY (media_id) REFERENCES media(id) ON DELETE CASCADE
) WITHOUT ROWID;

-- Covering index for genre -> album expansion in the mix builder
-- (media_relations.right_media_id IN (genres) returning left album ids), so the
-- lookup is satisfied from the index without touching the table.
CREATE INDEX IF NOT EXISTS idx_media_relations_right_left
    ON media_relations(right_media_id, left_media_id);
