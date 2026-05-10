-- Streams are ephemeral (resolved on demand from addons). Drop them so the
-- url column can be repurposed from URL strings to JSON descriptors.
DELETE FROM media WHERE kind = 'stream';
