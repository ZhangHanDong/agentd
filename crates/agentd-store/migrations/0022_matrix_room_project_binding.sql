ALTER TABLE matrix_bridge_rooms ADD COLUMN project_id TEXT;
CREATE INDEX idx_matrix_bridge_rooms_project ON matrix_bridge_rooms(project_id);
UPDATE schema_meta SET value = '22' WHERE key = 'version';
