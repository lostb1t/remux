-- DateCreated sorts order by `datetime(created_at)` (DATE_CREATED_ORDER_EXPR in
-- src/db/media.rs). It is the default sort of /items/latest, and without a
-- ParentId or IncludeItemTypes that query has no WHERE clause, so SQLite scans
-- and sorts the whole media table to return LIMIT rows. An index on the same
-- expression lets it walk the index and stop after LIMIT rows. Keep the two
-- textually identical or the planner will not use it.
CREATE INDEX IF NOT EXISTS idx_media_created_at_sort
    ON media(datetime(created_at));
