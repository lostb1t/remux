-- DateCreated / "/items/latest" sort acceleration.
--
-- `get_by_filter` orders the DateCreated sort by `datetime(created_at)`, and
-- there was no index on that expression, so the query did a full table SCAN of
-- `media` plus a temp b-tree sort (≈150–280 ms on a large library for
-- `/items?sortBy=DateCreated` and `/items/latest`).
--
-- This composite expression index matches the ORDER BY exactly
-- (`datetime(created_at)` then `id`, the deterministic tiebreaker), so SQLite
-- traverses it in order — forward for ASC, backward for DESC — with no temp
-- b-tree. The trailing `id` makes the ordering a stable total order, which also
-- fixes non-deterministic pagination across equal-timestamp rows.
CREATE INDEX IF NOT EXISTS idx_media_created_at_id
    ON media(datetime(created_at), id);

-- Second index leading with `kind` for the *type-filtered* latest paths
-- (e.g. `/items/latest?includeItemTypes=Series|Movie`, which add
-- `kind IN (<single type>)`). Without it, the planner would use the plain
-- index above and walk the whole created_at order filtering by kind per row —
-- scanning past every other type to reach a sparse one — which is far slower
-- than filtering by kind first. With `kind` leading, a single-type query gets
-- both the filter and the in-order scan from one index (no temp b-tree). The
-- plain index still serves the unfiltered case, where a kind-leading index
-- cannot provide global created_at order. Multi-type queries fall back to the
-- existing kind index + a sort over the (small) filtered subset, unchanged.
CREATE INDEX IF NOT EXISTS idx_media_kind_created_at_id
    ON media(kind, datetime(created_at), id);
