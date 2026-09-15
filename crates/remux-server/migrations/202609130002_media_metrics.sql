-- RemuxDB supplies one current score for each metric. These are not local
-- time-series aggregates, so store exactly one canonical row per media item.
CREATE TABLE media_metrics (
    media_id                BLOB PRIMARY KEY REFERENCES media(id) ON DELETE CASCADE,
    popularity_all_time     REAL,
    popularity_daily        REAL,
    popularity_weekly       REAL,
    popularity_monthly      REAL,
    popularity_yearly       REAL,
    trending_weekly         REAL,
    trending_monthly        REAL,
    synced_at               TEXT NOT NULL
);

-- Each indexed scan is already in the requested sort order. The item id keeps
-- the index covering for the join back to the filtered media result.
CREATE INDEX idx_media_metrics_popularity_all_time
    ON media_metrics(popularity_all_time DESC, media_id)
    WHERE popularity_all_time IS NOT NULL;
CREATE INDEX idx_media_metrics_popularity_daily
    ON media_metrics(popularity_daily DESC, media_id)
    WHERE popularity_daily IS NOT NULL;
CREATE INDEX idx_media_metrics_popularity_weekly
    ON media_metrics(popularity_weekly DESC, media_id)
    WHERE popularity_weekly IS NOT NULL;
CREATE INDEX idx_media_metrics_popularity_monthly
    ON media_metrics(popularity_monthly DESC, media_id)
    WHERE popularity_monthly IS NOT NULL;
CREATE INDEX idx_media_metrics_trending_weekly
    ON media_metrics(trending_weekly DESC, media_id)
    WHERE trending_weekly IS NOT NULL;
CREATE INDEX idx_media_metrics_trending_monthly
    ON media_metrics(trending_monthly DESC, media_id)
    WHERE trending_monthly IS NOT NULL;

DROP TABLE popularity_agg;
