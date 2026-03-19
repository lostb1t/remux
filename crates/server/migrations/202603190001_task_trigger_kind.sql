UPDATE task_triggers SET kind = 'DailyTrigger'  WHERE kind = 'schedule';
UPDATE task_triggers SET kind = 'StartupTrigger' WHERE kind = 'startup';
