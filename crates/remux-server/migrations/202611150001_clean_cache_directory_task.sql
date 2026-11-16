-- "Clean Cache Directory" (Jellyfin's DeleteCacheFiles) cleans the image and
-- subtitle caches daily; "Clear Cache" now wipes both on demand, so
-- "Clear Image Cache" goes.
DELETE FROM task_triggers WHERE task_id IN ('ClearImageCache', 'CleanSubtitleCache');
DELETE FROM task_results WHERE task_id IN ('ClearImageCache', 'CleanSubtitleCache');
DELETE FROM tasks WHERE id IN ('ClearImageCache', 'CleanSubtitleCache');
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron) VALUES
    ('default-deletecachefiles-daily', 'DeleteCacheFiles', 'DailyTrigger', NULL, '0 0 2 * * *');
