INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron)
SELECT 'default-refreshiptv-daily', 'RefreshIptv', 'DailyTrigger', NULL, '0 0 3 * * *'
WHERE NOT EXISTS (
    SELECT 1 FROM task_triggers WHERE LOWER(task_id) = 'refreshiptv'
);
