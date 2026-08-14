-- Shrink the default CleanTranscodeFolder interval from 24h to 4h so
-- orphaned Torznab/rqbit torrent downloads (see #226) don't sit around for
-- up to a full day on installs that don't get the eager on-stop sweep a
-- chance to run (e.g. a session that never reports a clean stop).
-- Only touches rows still on the shipped default, so an operator's own
-- customized interval is left untouched.
UPDATE task_triggers
SET cron = '0 0 */4 * * *'
WHERE task_id = 'CleanTranscodeFolder'
  AND kind = 'IntervalTrigger'
  AND cron = '0 0 */24 * * *';
