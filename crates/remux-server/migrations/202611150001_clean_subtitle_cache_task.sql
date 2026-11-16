-- Daily run of the new "Clean Subtitle Cache" task (deletes extracted
-- subtitles older than 30 days from DATA_DIR/subtitle-cache).
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron) VALUES
    ('default-cleansubtitlecache-daily', 'CleanSubtitleCache', 'DailyTrigger', NULL, '0 0 2 * * *');
