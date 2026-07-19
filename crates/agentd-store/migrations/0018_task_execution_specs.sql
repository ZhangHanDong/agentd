-- Versioned execution input owned by the task, not reconstructed by workers.
ALTER TABLE task_runs ADD COLUMN execution_spec_json TEXT;
UPDATE schema_meta SET value = '18' WHERE key = 'version';
