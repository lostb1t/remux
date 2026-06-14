-- Rename the "eclipse" preset kind to "monochrome" to match MonochromePreset.
UPDATE addons
SET preset = json_set(preset, '$.kind', 'monochrome')
WHERE json_extract(preset, '$.kind') = 'eclipse';
