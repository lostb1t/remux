-- Fix missing external_ids on the default Playlists row inserted by 202605310001.
UPDATE media SET external_ids = '{}' WHERE id = x'211d58cdf8504c74b4866f15480ebcc3' AND (external_ids IS NULL OR external_ids = '');
