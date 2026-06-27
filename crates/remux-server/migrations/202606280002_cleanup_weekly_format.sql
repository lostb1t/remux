-- Remove stale weekly rows that used the old 'YYYY-WNN' period_key format.
-- These cannot be parsed by strftime, causing NULL constraint failures in the
-- monthly rollup. Rows with the new 'YYYY-MM-DD' format (length = 10) are kept.
-- All derived periods (monthly, yearly, all) are also purged so they get
-- recomputed correctly from the cleaned-up weekly rows on the next task run.
DELETE FROM popularity_agg WHERE period = 'weekly' AND length(period_key) != 10;
DELETE FROM popularity_agg WHERE period IN ('monthly', 'yearly', 'all');
