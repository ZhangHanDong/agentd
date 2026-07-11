-- P221 / agent-chat parity Phase D: durable local attachment metadata.

ALTER TABLE direct_messages
ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]';

ALTER TABLE group_messages
ADD COLUMN attachments_json TEXT NOT NULL DEFAULT '[]';

UPDATE schema_meta SET value = '7' WHERE key = 'version';
