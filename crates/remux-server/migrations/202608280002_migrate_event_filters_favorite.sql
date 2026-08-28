-- Split the old "favorite" event filter into "mark_favorite" + "unmark_favorite".
-- event_filters is stored as a compact JSON array of snake_case strings.
-- A simple string replace is safe: "favorite" only appears as a standalone array
-- element, never as a substring of another variant name.
UPDATE user_media_trackers
SET event_filters = replace(event_filters, '"favorite"', '"mark_favorite","unmark_favorite"')
WHERE event_filters LIKE '%"favorite"%';
