DELETE FROM media WHERE kind = 'tv_program' AND live_end < datetime('now', '-1 day');
