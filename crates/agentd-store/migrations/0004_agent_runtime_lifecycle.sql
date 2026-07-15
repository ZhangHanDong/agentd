-- P214 / Phase C: minimal runtime lifecycle observation state.
ALTER TABLE agents ADD COLUMN runtime_state TEXT NOT NULL DEFAULT '{}';

UPDATE agents
SET runtime_state = '{}'
WHERE runtime_state IS NULL OR runtime_state = '';

UPDATE schema_meta SET value = '4' WHERE key = 'version';
