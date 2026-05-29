-- Patch iptv-m3u addon configs that are missing the epg_url key.
-- Safe to run multiple times: only touches rows where epg_url is absent from config.
-- The epg_url was dropped from iptv_sources before the table was removed,
-- so we cannot recover it here — this just ensures the key exists (as empty string)
-- so the addon config is well-formed.
UPDATE addons
SET preset = json_patch(
    preset,
    json_object(
        'config',
        json_set(json_extract(preset, '$.config'), '$.epg_url', '')
    )
)
WHERE json_extract(preset, '$.kind') = 'iptv-m3u'
  AND json_extract(preset, '$.config.epg_url') IS NULL;
