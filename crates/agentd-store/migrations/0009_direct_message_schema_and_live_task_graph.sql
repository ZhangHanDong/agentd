-- P227 / agent-chat replacement Phase D: preserve direct-message schema
-- metadata so live task-graph dispatch/result messages can round-trip.

ALTER TABLE direct_messages ADD COLUMN schema_json TEXT;

UPDATE schema_meta SET value = '9' WHERE key = 'version';
