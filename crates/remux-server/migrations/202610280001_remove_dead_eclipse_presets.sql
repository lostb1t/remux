-- Monochrome and SpotiFLAC are gone as presets; both were plain Eclipse addons.
-- Instances still on their bundled manifest (blank, or the stored default the
-- dashboard saves) no longer work, so remove them, including the Monochrome row
-- seeded on fresh installs. Instances with their own manifest_url move to the
-- generic Eclipse preset, which takes the same config.
DELETE FROM addons
WHERE json_extract(preset, '$.kind') IN ('monochrome', 'eclipse_spotiflac')
  AND COALESCE(trim(json_extract(preset, '$.config.manifest_url')), '') IN (
      '',
      'https://monochrome1.cyrusna29.workers.dev/u/206f62ce5c9a5c710f2178a16238/manifest.json',
      'https://spotiflac.eclipsemusic.app/5baa7290b334d6e2/manifest.json'
  );

UPDATE addons
SET preset = json_set(preset, '$.kind', 'eclipse')
WHERE json_extract(preset, '$.kind') IN ('monochrome', 'eclipse_spotiflac');
