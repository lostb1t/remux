-- A smart collection of the requesting user's own watchlist. The rule is
-- evaluated per user, so everyone sees their own items and an empty watchlist
-- hides the collection. Promoted, so it is a library view of its own and also
-- listed with the other collections (clients like Plezy show it either way).
INSERT OR IGNORE INTO media (id, title, kind, collection_kind, collection_media_kind, collection_smart_filter, collection_default_sort, collection_default_sort_order, collection_latest_auto_unplayed, collection_latest_sort_digital, external_ids, locked_fields, promoted, enabled, sort_order, created_at, updated_at) VALUES
    (x'a1b2c3d4000040008000000000000004', 'Watchlist', 'collection', 'smart', 'mixed', '{"match_mode":"all","groups":[{"match_mode":"all","rules":[{"field":"watchlist","value":true}]}]}', '["DateWatchlisted"]', '["Descending"]', 0, 0, '{}', '[]', 1, 1, 12, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP);
