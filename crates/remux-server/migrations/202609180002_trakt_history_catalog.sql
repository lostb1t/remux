-- A single smart view is shared safely by every user: its Tracked rule is
-- evaluated with the requesting user's state, while the tag limits it to
-- roots materialized by a Trakt import.
INSERT OR IGNORE INTO media (
    id,
    title,
    kind,
    collection_kind,
    collection_media_kind,
    collection_smart_filter,
    collection_default_sort,
    collection_default_sort_order,
    collection_latest_auto_unplayed,
    collection_latest_sort_digital,
    collection_image_config,
    external_ids,
    is_locked,
    locked_fields,
    promoted,
    enabled,
    sort_order,
    created_at,
    updated_at
) VALUES (
    X'7472616b742d4c696272617279000001',
    'Trakt History',
    'collection',
    'smart',
    'mixed',
    '{"match_mode":"all","groups":[{"match_mode":"all","rules":[{"field":"tag","op":"in","values":["source:Trakt"]},{"field":"tracked","value":true}]}]}',
    '["DatePlayed"]',
    '["Descending"]',
    0,
    0,
    '{}',
    '{}',
    0,
    '[]',
    1,
    1,
    12,
    CURRENT_TIMESTAMP,
    CURRENT_TIMESTAMP
);
