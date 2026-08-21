-- Rename the queue's first delivery kind from 'tracker' to 'media_tracker'.
--
-- 'tracker' is also what this codebase calls a torrent announce URL, so the
-- column read as the wrong thing at a glance. The CHECK that ties a row to
-- its owner names the value, and SQLite cannot alter a constraint, so the
-- table is rebuilt rather than updated in place. Nothing references
-- delivery_queue, and no released version writes to it, so this is a copy of
-- an empty table on every install that already has one.
CREATE TABLE delivery_queue_new (
    id                    BLOB PRIMARY KEY NOT NULL,
    -- media_tracker
    kind                  TEXT NOT NULL,
    -- The owner a 'media_tracker' row belongs to, so disconnecting takes its
    -- backlog with it. Each kind gets its own nullable owner column: SQLite
    -- foreign keys can only name one table, and a trigger would not fire when
    -- the delete arrives via a cascade from users.
    user_media_tracker_id BLOB
                          REFERENCES user_media_trackers(id) ON DELETE CASCADE,
    -- The kind's body as JSON. For 'media_tracker': the MediaTrackerEvent and
    -- the id of the item it was about, which is resolved at delivery.
    payload               TEXT NOT NULL,
    -- pending | delivered | failed_retryable | failed_permanent
    status                TEXT NOT NULL DEFAULT 'pending',
    attempts              INTEGER NOT NULL DEFAULT 0,
    next_attempt_at       DATETIME NOT NULL,
    last_error            TEXT,
    created_at            DATETIME NOT NULL,
    updated_at            DATETIME NOT NULL,
    delivered_at          DATETIME,

    -- A kind is only ever readable with its owner present.
    CHECK (kind <> 'media_tracker' OR user_media_tracker_id IS NOT NULL)
);

INSERT INTO delivery_queue_new
SELECT id,
       CASE kind WHEN 'tracker' THEN 'media_tracker' ELSE kind END,
       user_media_tracker_id,
       payload,
       status,
       attempts,
       next_attempt_at,
       last_error,
       created_at,
       updated_at,
       delivered_at
FROM delivery_queue;

DROP TABLE delivery_queue;

ALTER TABLE delivery_queue_new RENAME TO delivery_queue;

-- The worker's claim query: due rows, oldest first.
CREATE INDEX idx_delivery_queue_due
    ON delivery_queue(status, next_attempt_at);

-- Backs the per-media-tracker activity view and the cascade on disconnect.
CREATE INDEX idx_delivery_queue_user_media_tracker_id
    ON delivery_queue(user_media_tracker_id);
