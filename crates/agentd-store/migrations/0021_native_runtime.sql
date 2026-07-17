-- AD-E5 native PTY runtime metadata, immutable semantic events, redacted
-- transcript objects, digest-only input actions, and restart recovery history.

CREATE TABLE runtime_transcript_objects (
    id                    TEXT PRIMARY KEY CHECK (
                              length(id) = 29
                              AND substr(id, 1, 3) = 'rx_'
                              AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                          ),
    runtime_session_id    TEXT NOT NULL REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    runtime_attempt_id    TEXT NOT NULL UNIQUE REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    content_sha256        TEXT NOT NULL CHECK (
                              length(content_sha256) = 64
                              AND content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                          ),
    storage_ref           TEXT NOT NULL CHECK (storage_ref GLOB 'sha256:*'),
    size_bytes            INTEGER NOT NULL CHECK (size_bytes >= 0),
    truncated             INTEGER NOT NULL CHECK (truncated IN (0, 1)),
    archived_at           INTEGER NOT NULL,
    UNIQUE (runtime_session_id, runtime_attempt_id, content_sha256)
);

ALTER TABLE runtime_sessions ADD COLUMN provider TEXT
    CHECK (provider IS NULL OR provider IN ('codex', 'claude_code', 'custom'));
ALTER TABLE runtime_sessions ADD COLUMN command_sha256 TEXT
    CHECK (command_sha256 IS NULL OR (
        length(command_sha256) = 64
        AND command_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ));
ALTER TABLE runtime_sessions ADD COLUMN sandbox_id TEXT;
ALTER TABLE runtime_sessions ADD COLUMN sandbox_profile_sha256 TEXT
    CHECK (sandbox_profile_sha256 IS NULL OR (
        length(sandbox_profile_sha256) = 64
        AND sandbox_profile_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ));
ALTER TABLE runtime_sessions ADD COLUMN sandbox_expires_at INTEGER;
ALTER TABLE runtime_sessions ADD COLUMN max_capture_bytes INTEGER
    CHECK (max_capture_bytes IS NULL OR max_capture_bytes > 0);
ALTER TABLE runtime_sessions ADD COLUMN max_transcript_bytes INTEGER
    CHECK (max_transcript_bytes IS NULL OR max_transcript_bytes > 0);
ALTER TABLE runtime_sessions ADD COLUMN idle_timeout_ms INTEGER
    CHECK (idle_timeout_ms IS NULL OR idle_timeout_ms > 0);
ALTER TABLE runtime_sessions ADD COLUMN current_attempt_id TEXT
    REFERENCES runtime_attempts(id) ON DELETE RESTRICT;
ALTER TABLE runtime_sessions ADD COLUMN native_session_ref TEXT;
ALTER TABLE runtime_sessions ADD COLUMN transcript_id TEXT
    REFERENCES runtime_transcript_objects(id) ON DELETE RESTRICT;

ALTER TABLE runtime_attempts ADD COLUMN host_instance_id TEXT;
ALTER TABLE runtime_attempts ADD COLUMN exit_code INTEGER;
ALTER TABLE runtime_attempts ADD COLUMN transcript_id TEXT
    REFERENCES runtime_transcript_objects(id) ON DELETE RESTRICT;

CREATE TABLE native_runtime_events (
    id                    TEXT PRIMARY KEY CHECK (
                              length(id) = 29
                              AND substr(id, 1, 3) = 're_'
                              AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                          ),
    runtime_session_id    TEXT NOT NULL REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    runtime_attempt_id    TEXT NOT NULL REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    event_index           INTEGER NOT NULL CHECK (event_index > 0),
    kind                  TEXT NOT NULL CHECK (kind IN (
                              'starting', 'started', 'output', 'input_accepted', 'resized',
                              'interrupted', 'native_session_ref', 'exited', 'runtime_gone',
                              'recovered', 'transcript_archived', 'shutdown'
                          )),
    payload_sha256        TEXT NOT NULL CHECK (
                              length(payload_sha256) = 64
                              AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                          ),
    payload_json          TEXT NOT NULL CHECK (json_valid(payload_json)),
    occurred_at           INTEGER NOT NULL,
    UNIQUE (runtime_session_id, event_index)
);

CREATE INDEX idx_native_runtime_events_attempt
    ON native_runtime_events(runtime_attempt_id, event_index);

CREATE TABLE native_runtime_input_actions (
    runtime_attempt_id    TEXT NOT NULL REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    idempotency_key       TEXT NOT NULL,
    input_sha256          TEXT NOT NULL CHECK (
                              length(input_sha256) = 64
                              AND input_sha256 NOT GLOB '*[^0123456789abcdef]*'
                          ),
    byte_count            INTEGER NOT NULL CHECK (byte_count > 0),
    accepted_at           INTEGER NOT NULL,
    runtime_event_id      TEXT NOT NULL UNIQUE REFERENCES native_runtime_events(id) ON DELETE RESTRICT,
    PRIMARY KEY (runtime_attempt_id, idempotency_key)
);

CREATE TABLE native_runtime_recovery_history (
    sequence              INTEGER PRIMARY KEY AUTOINCREMENT,
    runtime_session_id    TEXT NOT NULL REFERENCES runtime_sessions(id) ON DELETE RESTRICT,
    previous_attempt_id   TEXT NOT NULL REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    next_attempt_id       TEXT REFERENCES runtime_attempts(id) ON DELETE RESTRICT,
    disposition           TEXT NOT NULL CHECK (disposition IN ('live', 'resumable', 'runtime_gone')),
    record_sha256         TEXT NOT NULL UNIQUE CHECK (
                              length(record_sha256) = 64
                              AND record_sha256 NOT GLOB '*[^0123456789abcdef]*'
                          ),
    record_json           TEXT NOT NULL CHECK (json_valid(record_json)),
    observed_at           INTEGER NOT NULL
);

CREATE INDEX idx_native_runtime_recovery_session
    ON native_runtime_recovery_history(runtime_session_id, sequence);

CREATE TRIGGER trg_runtime_transcripts_no_update BEFORE UPDATE ON runtime_transcript_objects
BEGIN SELECT RAISE(ABORT, 'runtime transcript objects are immutable'); END;
CREATE TRIGGER trg_runtime_transcripts_no_delete BEFORE DELETE ON runtime_transcript_objects
BEGIN SELECT RAISE(ABORT, 'runtime transcript objects are immutable'); END;
CREATE TRIGGER trg_native_runtime_events_no_update BEFORE UPDATE ON native_runtime_events
BEGIN SELECT RAISE(ABORT, 'native runtime events are immutable'); END;
CREATE TRIGGER trg_native_runtime_events_no_delete BEFORE DELETE ON native_runtime_events
BEGIN SELECT RAISE(ABORT, 'native runtime events are immutable'); END;
CREATE TRIGGER trg_native_runtime_inputs_no_update BEFORE UPDATE ON native_runtime_input_actions
BEGIN SELECT RAISE(ABORT, 'native runtime input actions are immutable'); END;
CREATE TRIGGER trg_native_runtime_inputs_no_delete BEFORE DELETE ON native_runtime_input_actions
BEGIN SELECT RAISE(ABORT, 'native runtime input actions are immutable'); END;
CREATE TRIGGER trg_native_runtime_recovery_no_update BEFORE UPDATE ON native_runtime_recovery_history
BEGIN SELECT RAISE(ABORT, 'native runtime recovery history is immutable'); END;
CREATE TRIGGER trg_native_runtime_recovery_no_delete BEFORE DELETE ON native_runtime_recovery_history
BEGIN SELECT RAISE(ABORT, 'native runtime recovery history is immutable'); END;

UPDATE schema_meta SET value = '21' WHERE key = 'version';
