-- P213 / Phase C: agent-chat compatible local agent registry baseline.
ALTER TABLE agents ADD COLUMN name TEXT;
ALTER TABLE agents ADD COLUMN capability TEXT;
ALTER TABLE agents ADD COLUMN runtime TEXT;
ALTER TABLE agents ADD COLUMN model TEXT;
ALTER TABLE agents ADD COLUMN tmux_target TEXT;
ALTER TABLE agents ADD COLUMN home_dir TEXT;
ALTER TABLE agents ADD COLUMN workdir TEXT;
ALTER TABLE agents ADD COLUMN state_dir TEXT;
ALTER TABLE agents ADD COLUMN server TEXT;
ALTER TABLE agents ADD COLUMN status TEXT NOT NULL DEFAULT 'offline';
ALTER TABLE agents ADD COLUMN offline_reason TEXT;
ALTER TABLE agents ADD COLUMN last_seen_at INTEGER;
ALTER TABLE agents ADD COLUMN registered_at INTEGER;
ALTER TABLE agents ADD COLUMN updated_at INTEGER;
ALTER TABLE agents ADD COLUMN runtime_profile TEXT NOT NULL DEFAULT '{}';

UPDATE agents
SET name = id
WHERE name IS NULL OR name = '';

UPDATE agents
SET registered_at = created_at
WHERE registered_at IS NULL;

UPDATE agents
SET updated_at = created_at
WHERE updated_at IS NULL;

UPDATE agents
SET status = CASE WHEN enabled = 1 THEN 'offline' ELSE 'disabled' END
WHERE status IS NULL OR status = '';

UPDATE agents
SET runtime_profile = '{}'
WHERE runtime_profile IS NULL OR runtime_profile = '';

CREATE UNIQUE INDEX idx_agents_name ON agents(name) WHERE name IS NOT NULL;

UPDATE schema_meta SET value = '3' WHERE key = 'version';
