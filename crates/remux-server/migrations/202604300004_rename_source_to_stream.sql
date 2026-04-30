-- Rename MediaKind::Source → MediaKind::Stream internally.
-- The DB string changes from "source" to "stream"; API-facing field names are
-- unchanged (MediaSources, probe_data, etc. are column/struct names, not kind values).
UPDATE media SET kind = 'stream' WHERE kind = 'source';
