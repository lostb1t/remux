-- Backfill media_id from old Stremio-format keys before recreating table

-- Movies and Series ("tt...")
UPDATE user_media_state
SET media_id = (
    SELECT m.id FROM media m
    WHERE json_extract(m.external_ids, '$.imdb') = media_key
      AND m.kind IN ('movie', 'series')
    LIMIT 1
)
WHERE media_id IS NULL
  AND media_key LIKE 'tt%'
  AND instr(media_key, ':') = 0;

-- Seasons ("tt...:N")
UPDATE user_media_state
SET media_id = (
    SELECT s.id FROM media s
    JOIN media ser ON ser.id = s.parent_id
    WHERE json_extract(ser.external_ids, '$.imdb') = substr(media_key, 1, instr(media_key, ':') - 1)
      AND ser.kind = 'series' AND s.kind = 'season'
      AND s.idx = CAST(substr(media_key, instr(media_key, ':') + 1) AS INTEGER)
    LIMIT 1
)
WHERE media_id IS NULL
  AND media_key LIKE 'tt%'
  AND instr(media_key, ':') > 0
  AND instr(substr(media_key, instr(media_key, ':') + 1), ':') = 0;

-- Episodes ("tt...:S:E") — match via series_imdb on episode rows
UPDATE user_media_state
SET media_id = (
    SELECT m.id FROM media m
    WHERE json_extract(m.external_ids, '$.series_imdb') = substr(media_key, 1, instr(media_key, ':') - 1)
      AND m.kind = 'episode'
      AND m.parent_idx = CAST(
            substr(substr(media_key, instr(media_key, ':') + 1), 1,
              instr(substr(media_key, instr(media_key, ':') + 1), ':') - 1) AS INTEGER)
      AND m.idx = CAST(
            substr(substr(media_key, instr(media_key, ':') + 1),
              instr(substr(media_key, instr(media_key, ':') + 1), ':') + 1) AS INTEGER)
    LIMIT 1
)
WHERE media_id IS NULL
  AND media_key LIKE 'tt%'
  AND instr(media_key, ':') > 0
  AND instr(substr(media_key, instr(media_key, ':') + 1), ':') > 0;

-- UUID hex format — already resolved in a previous run
UPDATE user_media_state
SET media_id = (
    SELECT m.id FROM media m WHERE lower(hex(m.id)) = media_key LIMIT 1
)
WHERE media_id IS NULL
  AND media_key NOT LIKE 'tt%'
  AND length(media_key) = 32;

-- Recreate table: media_id as NOT NULL primary key, media_key renamed to media_raw
CREATE TABLE user_media_state_new (
    user_id           BLOB NOT NULL,
    media_id          BLOB NOT NULL,
    media_raw         TEXT,
    favorite          INT NOT NULL DEFAULT 0,
    play_count        INT NOT NULL DEFAULT 0,
    played_at         DATETIME,
    playback_position INT NOT NULL DEFAULT 0,
    stream_id         BLOB,
    subtitle_idx      INT,
    audio_idx         INT,
    last_played_at    DATETIME,
    PRIMARY KEY (user_id, media_id)
);

-- Copy rows with resolved media_id; orphaned rows (unsynced content) are dropped.
-- Old media_key value carried over into media_raw as a legacy reference.
INSERT INTO user_media_state_new
    (user_id, media_id, media_raw, favorite, play_count, played_at,
     playback_position, stream_id, subtitle_idx, audio_idx, last_played_at)
SELECT user_id, media_id, media_key, favorite, play_count, played_at,
       playback_position, stream_id, subtitle_idx, audio_idx, last_played_at
FROM user_media_state
WHERE media_id IS NOT NULL;

DROP TABLE user_media_state;
ALTER TABLE user_media_state_new RENAME TO user_media_state;
