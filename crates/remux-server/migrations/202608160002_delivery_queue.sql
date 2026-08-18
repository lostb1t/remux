-- Outbound deliveries waiting to leave the server, one row per attempt target.
--
-- Written synchronously by the request handler before it returns, so a
-- scrobble survives a crash, a restart, or the provider being down. An
-- in-memory channel would drop these on lag.
--
-- `kind` says who delivers the row and how `payload` is read; the retry state
-- machine below it is shared. Tracking is the first kind, webhooks the next.
CREATE TABLE delivery_queue (
    id                    BLOB PRIMARY KEY NOT NULL,
    -- tracker (more to come: webhook)
    kind                  TEXT NOT NULL,
    -- The owner a 'tracker' row belongs to, so disconnecting takes its backlog
    -- with it. Each kind gets its own nullable owner column: SQLite foreign
    -- keys can only name one table, and a trigger would not fire when the
    -- delete arrives via a cascade from users.
    user_media_tracker_id BLOB
                          REFERENCES user_media_trackers(id) ON DELETE CASCADE,
    -- The kind's body as JSON. For 'tracker': the TrackingEvent plus the
    -- resolved TrackingTarget, so delivery never has to re-resolve the item.
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
    CHECK (kind <> 'tracker' OR user_media_tracker_id IS NOT NULL)
);

-- The worker's claim query: due rows, oldest first.
CREATE INDEX idx_delivery_queue_due
    ON delivery_queue(status, next_attempt_at);

-- Backs the per-media-tracker activity view and the cascade on disconnect.
CREATE INDEX idx_delivery_queue_user_media_tracker_id
    ON delivery_queue(user_media_tracker_id);

-- Without a trigger the task only ever runs when an admin presses the button,
-- so a retryable failure would never actually be retried. Handlers also poke
-- the worker on enqueue; this sweep is what catches backoffs coming due and
-- anything left queued across a restart.
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron)
VALUES ('default-deliveryqueuesync-interval', 'DeliveryQueueSync',
        'IntervalTrigger', NULL, '0 */5 * * * *');
