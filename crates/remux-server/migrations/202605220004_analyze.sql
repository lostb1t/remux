-- Update query planner statistics so SQLite correctly prefers PK lookups for small
-- IN lists over full index scans. Without this, the planner over-estimates the cost
-- of primary-key lookups relative to expression-index range scans on large tables.
ANALYZE;
