-- RemuxDB is now the canonical source for popularity, trending, and ratings.
-- Discard scores derived from the retired per-addon metric implementations.
DELETE FROM popularity_agg;
DROP TABLE IF EXISTS popularity_raw;
