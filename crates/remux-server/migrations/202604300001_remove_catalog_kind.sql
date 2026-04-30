-- Remove all catalog media rows — catalog state now lives in addon config.
-- Deezer playlist IDs were already migrated to the deezer addon row config
-- by the Rust startup migration (addons/migrate.rs), so no inline preservation
-- is needed here.
DELETE FROM media WHERE kind = 'catalog';
