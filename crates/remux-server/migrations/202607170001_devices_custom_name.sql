-- Add a user-settable custom display name to devices, matching Jellyfin's
-- DeviceOptionsDto.CustomName (surfaced by the admin dashboard's Devices page).
-- Nullable: NULL means "fall back to the client-reported device name".
ALTER TABLE devices ADD COLUMN custom_name TEXT;
