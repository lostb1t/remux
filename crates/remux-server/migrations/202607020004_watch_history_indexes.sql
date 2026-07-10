-- Indexes for recommendation and interest-path lookups on watch history.
-- User-scoped recommendation queries commonly filter on (user_id, event_type)
-- and order by created_at; this index avoids scanning unrelated events.
CREATE INDEX IF NOT EXISTS idx_watch_history_user_event_created
    ON watch_history(user_id, event_type, created_at DESC);

-- Global event-type scans for admin/debug tooling and batch tasks.
CREATE INDEX IF NOT EXISTS idx_watch_history_event_created
    ON watch_history(event_type, created_at DESC);

