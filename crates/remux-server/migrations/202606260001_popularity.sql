CREATE TABLE IF NOT EXISTS popularity_raw (
    source       TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    value        REAL NOT NULL,
    date         TEXT NOT NULL,
    PRIMARY KEY (source, external_id, date)
);

CREATE TABLE IF NOT EXISTS popularity_agg (
    source       TEXT NOT NULL,
    external_id  TEXT NOT NULL,
    period       TEXT NOT NULL,
    period_key   TEXT NOT NULL,
    avg          REAL NOT NULL,
    min          REAL NOT NULL,
    max          REAL NOT NULL,
    sample_count INTEGER NOT NULL,
    PRIMARY KEY (source, external_id, period, period_key)
);

CREATE INDEX IF NOT EXISTS idx_pop_agg_lookup
    ON popularity_agg(source, period, period_key, avg DESC);

CREATE INDEX IF NOT EXISTS idx_pop_raw_date
    ON popularity_raw(source, date);

INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron) VALUES
    ('default-refreshpopularity-daily', 'RefreshPopularity', 'DailyTrigger', NULL, '0 0 10 * * *');
