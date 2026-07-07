-- P108 / P2 C2: distinguish Delphi review rounds at the park payload layer.
ALTER TABLE review_runs ADD COLUMN round INTEGER NOT NULL DEFAULT 1;

UPDATE schema_meta SET value = '2' WHERE key = 'version';
