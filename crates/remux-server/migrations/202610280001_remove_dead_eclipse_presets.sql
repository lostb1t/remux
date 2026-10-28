-- Monochrome and SpotiFLAC no longer work. Remove the instances that only ever
-- used the bundled default manifest (including the Monochrome row seeded on
-- fresh installs). Instances with their own manifest_url are left alone: those
-- still load, they are just no longer offered for new addons.
DELETE FROM addons
WHERE json_extract(preset, '$.kind') IN ('monochrome', 'eclipse_spotiflac')
  AND COALESCE(trim(json_extract(preset, '$.config.manifest_url')), '') = '';
