-- Default 12h recurring trigger for RefreshDispatcharrLiveTv, so channel/
-- stream titles (which embed a resolution tag computed from Dispatcharr's
-- stream_stats) stay current as underlying stream health drifts.
INSERT OR IGNORE INTO task_triggers (id, task_id, kind, time_limit_hours, cron)
VALUES ('default-dispatcharrlivetv-interval', 'RefreshDispatcharrLiveTv',
        'IntervalTrigger', NULL, '0 0 */12 * * *');
