-- Pending tracking deliveries, one row per (event, connection).
--
-- Written synchronously by the request handler before it returns, so a
-- scrobble survives a crash, a restart, or the provider being down. An
-- in-memory channel would drop these on lag.
CREATE TABLE tracking_outbox (
    id                  BLOB PRIMARY KEY NOT NULL,
    user_media_tracker_id BLOB NOT NULL REFERENCES user_media_trackers(id) ON DELETE CASCADE,
    event_kind          TEXT NOT NULL,
    -- Serialised TrackingEvent plus the resolved TrackingTarget, so delivery
    -- never has to re-resolve the item.
    payload             TEXT NOT NULL,
    -- pending | delivered | failed_retryable | failed_permanent
    status              TEXT NOT NULL DEFAULT 'pending',
    attempts            INTEGER NOT NULL DEFAULT 0,
    next_attempt_at     DATETIME NOT NULL,
    last_error          TEXT,
    created_at          DATETIME NOT NULL,
    updated_at          DATETIME NOT NULL,
    delivered_at        DATETIME
);

-- The worker's claim query: due rows, oldest first.
CREATE INDEX idx_tracking_outbox_due
    ON tracking_outbox(status, next_attempt_at);

-- Backs the per-connection activity view and the cascade on disconnect.
CREATE INDEX idx_tracking_outbox_connection
    ON tracking_outbox(user_media_tracker_id);

-- Without a trigger the task only ever runs when an admin presses the button,
-- so a retryable failure would never actually be retried. Handlers also poke
-- the worker on enqueue; this sweep is what catches backoffs coming due and
-- anything left queued across a restart.
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron) VALUES
    ('default-trackingsync-interval', 'TrackingSync', 'IntervalTrigger', NULL, '0 */5 * * * *');
