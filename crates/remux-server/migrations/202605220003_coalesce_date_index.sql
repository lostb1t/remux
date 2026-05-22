-- Expression index so COALESCE(digital_released_at, released_at) <= ? can use an index.
-- The separate released_at / digital_released_at indexes can't cover this expression.
-- Prefixed with kind so the planner can satisfy both the kind IN (?) and date filters
-- in one B-tree scan when the IN subquery is large or not used as the driving table.
CREATE INDEX IF NOT EXISTS idx_media_kind_available_date
    ON media(kind, COALESCE(digital_released_at, released_at));
