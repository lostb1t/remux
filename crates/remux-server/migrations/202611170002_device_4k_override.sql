-- An admin's choice for a device's 4K support, kept apart from what playback
-- has shown (is_4k_capable). NULL: decided by playback; 1: supports 4K;
-- 0: does not.
ALTER TABLE devices ADD COLUMN is_4k_capable_override INTEGER;
