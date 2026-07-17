-- AD-E6 durable final cutover, offline mappings, service records, and rollback evidence.

CREATE TABLE cutover_runs (
    id TEXT PRIMARY KEY CHECK (length(id) = 29 AND substr(id, 1, 3) = 'co_'),
    source_root_sha256 TEXT NOT NULL CHECK (
        length(source_root_sha256) = 64
        AND source_root_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ),
    target_database_sha256 TEXT CHECK (
        target_database_sha256 IS NULL OR (
            length(target_database_sha256) = 64
            AND target_database_sha256 NOT GLOB '*[^0123456789abcdef]*'
        )
    ),
    rollback_window_expires_at INTEGER NOT NULL,
    state TEXT NOT NULL CHECK (state IN (
        'planned','importing','shadowing','draining','handoff_ready','active','retired','rolled_back'
    )),
    source_id TEXT REFERENCES cutover_sources(id) ON DELETE RESTRICT,
    authority_owner TEXT NOT NULL CHECK (authority_owner IN ('agent_chat_read_only','agentd','none')),
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at INTEGER NOT NULL,
    updated_at INTEGER NOT NULL
);

CREATE TABLE cutover_sources (
    id TEXT PRIMARY KEY CHECK (length(id) = 29 AND substr(id, 1, 3) = 'sx_'),
    cutover_id TEXT NOT NULL UNIQUE REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    source_sha256 TEXT NOT NULL CHECK (
        length(source_sha256) = 64
        AND source_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ),
    file_count INTEGER NOT NULL CHECK (file_count >= 0),
    record_count INTEGER NOT NULL CHECK (record_count >= 0),
    captured_at INTEGER NOT NULL
);

CREATE TABLE cutover_transitions (
    sequence INTEGER PRIMARY KEY AUTOINCREMENT,
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    expected_state TEXT NOT NULL,
    next_state TEXT NOT NULL,
    idempotency_key TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    input_sha256 TEXT NOT NULL CHECK (
        length(input_sha256) = 64
        AND input_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ),
    authority_owner TEXT NOT NULL CHECK (authority_owner IN ('agent_chat_read_only','agentd','none')),
    occurred_at INTEGER NOT NULL,
    UNIQUE (cutover_id, idempotency_key)
);

CREATE TABLE cutover_id_mappings (
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    surface TEXT NOT NULL CHECK (surface IN (
        'agent','group','message','cursor','task','task_graph','matrix_project'
    )),
    legacy_id_sha256 TEXT NOT NULL CHECK (
        length(legacy_id_sha256) = 64
        AND legacy_id_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ),
    native_id TEXT NOT NULL CHECK (length(trim(native_id)) > 0),
    native_record_sha256 TEXT NOT NULL CHECK (
        length(native_record_sha256) = 64
        AND native_record_sha256 NOT GLOB '*[^0123456789abcdef]*'
    ),
    mapped_at INTEGER NOT NULL,
    PRIMARY KEY (cutover_id, surface, legacy_id_sha256),
    UNIQUE (cutover_id, surface, native_id)
);

CREATE TABLE cutover_shadow_decisions (
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    surface TEXT NOT NULL CHECK (surface IN (
        'agent','group','message','cursor','task','task_graph','matrix_project'
    )),
    decision_key_sha256 TEXT NOT NULL CHECK (length(decision_key_sha256) = 64),
    legacy_decision_sha256 TEXT NOT NULL CHECK (length(legacy_decision_sha256) = 64),
    native_decision_sha256 TEXT NOT NULL CHECK (length(native_decision_sha256) = 64),
    matched INTEGER NOT NULL CHECK (matched IN (0, 1)),
    reason_code TEXT NOT NULL CHECK (length(trim(reason_code)) > 0),
    observed_at INTEGER NOT NULL,
    PRIMARY KEY (cutover_id, surface, decision_key_sha256)
);

CREATE TABLE cutover_step_receipts (
    id TEXT PRIMARY KEY CHECK (length(id) = 29 AND substr(id, 1, 3) = 'ct_'),
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    step TEXT NOT NULL CHECK (length(trim(step)) > 0),
    idempotency_key TEXT NOT NULL CHECK (length(trim(idempotency_key)) > 0),
    input_sha256 TEXT NOT NULL CHECK (length(input_sha256) = 64),
    output_sha256 TEXT NOT NULL CHECK (length(output_sha256) = 64),
    occurred_at INTEGER NOT NULL,
    UNIQUE (cutover_id, step, idempotency_key)
);

CREATE TABLE cutover_cursor_handoffs (
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    project_ref_sha256 TEXT NOT NULL CHECK (length(project_ref_sha256) = 64),
    previous_cursor_sha256 TEXT NOT NULL CHECK (length(previous_cursor_sha256) = 64),
    next_cursor TEXT NOT NULL CHECK (length(trim(next_cursor)) > 0),
    authority_owner TEXT NOT NULL CHECK (authority_owner IN ('agent_chat_read_only','agentd','none')),
    acknowledged INTEGER NOT NULL CHECK (acknowledged IN (0, 1)),
    handed_off_at INTEGER NOT NULL,
    PRIMARY KEY (cutover_id, project_ref_sha256)
);

CREATE TABLE cutover_backup_manifests (
    id TEXT PRIMARY KEY CHECK (length(id) = 29 AND substr(id, 1, 3) = 'bm_'),
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    database_sha256 TEXT NOT NULL CHECK (length(database_sha256) = 64),
    schema_version INTEGER NOT NULL CHECK (schema_version > 0),
    size_bytes INTEGER NOT NULL CHECK (size_bytes >= 0),
    storage_ref TEXT NOT NULL CHECK (length(trim(storage_ref)) > 0),
    created_at INTEGER NOT NULL
);

CREATE TABLE cutover_service_installations (
    id TEXT PRIMARY KEY CHECK (length(id) = 29 AND substr(id, 1, 3) = 'sv_'),
    cutover_id TEXT NOT NULL REFERENCES cutover_runs(id) ON DELETE RESTRICT,
    model TEXT NOT NULL CHECK (model IN ('local','team','fleet')),
    manifest_sha256 TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
    target_ref_sha256 TEXT NOT NULL CHECK (length(target_ref_sha256) = 64),
    installed_at INTEGER NOT NULL,
    UNIQUE (cutover_id, model, target_ref_sha256)
);

CREATE TRIGGER cutover_state_transition_guard
BEFORE UPDATE OF state ON cutover_runs
WHEN OLD.state <> NEW.state AND NOT (
    (OLD.state = 'planned' AND NEW.state = 'importing') OR
    (OLD.state = 'importing' AND NEW.state IN ('shadowing','rolled_back')) OR
    (OLD.state = 'shadowing' AND NEW.state IN ('draining','rolled_back')) OR
    (OLD.state = 'draining' AND NEW.state IN ('handoff_ready','rolled_back')) OR
    (OLD.state = 'handoff_ready' AND NEW.state IN ('active','rolled_back')) OR
    (OLD.state = 'active' AND NEW.state IN ('retired','rolled_back'))
)
BEGIN SELECT RAISE(ABORT, 'illegal cutover state transition'); END;

CREATE TRIGGER cutover_sources_no_update BEFORE UPDATE ON cutover_sources BEGIN SELECT RAISE(ABORT, 'cutover sources are immutable'); END;
CREATE TRIGGER cutover_sources_no_delete BEFORE DELETE ON cutover_sources BEGIN SELECT RAISE(ABORT, 'cutover sources are immutable'); END;
CREATE TRIGGER cutover_transitions_no_update BEFORE UPDATE ON cutover_transitions BEGIN SELECT RAISE(ABORT, 'cutover transitions are immutable'); END;
CREATE TRIGGER cutover_transitions_no_delete BEFORE DELETE ON cutover_transitions BEGIN SELECT RAISE(ABORT, 'cutover transitions are immutable'); END;
CREATE TRIGGER cutover_mappings_no_update BEFORE UPDATE ON cutover_id_mappings BEGIN SELECT RAISE(ABORT, 'cutover mappings are immutable'); END;
CREATE TRIGGER cutover_mappings_no_delete BEFORE DELETE ON cutover_id_mappings BEGIN SELECT RAISE(ABORT, 'cutover mappings are immutable'); END;
CREATE TRIGGER cutover_shadow_no_update BEFORE UPDATE ON cutover_shadow_decisions BEGIN SELECT RAISE(ABORT, 'cutover shadow decisions are immutable'); END;
CREATE TRIGGER cutover_shadow_no_delete BEFORE DELETE ON cutover_shadow_decisions BEGIN SELECT RAISE(ABORT, 'cutover shadow decisions are immutable'); END;
CREATE TRIGGER cutover_receipts_no_update BEFORE UPDATE ON cutover_step_receipts BEGIN SELECT RAISE(ABORT, 'cutover receipts are immutable'); END;
CREATE TRIGGER cutover_receipts_no_delete BEFORE DELETE ON cutover_step_receipts BEGIN SELECT RAISE(ABORT, 'cutover receipts are immutable'); END;
CREATE TRIGGER cutover_handoffs_no_update BEFORE UPDATE ON cutover_cursor_handoffs BEGIN SELECT RAISE(ABORT, 'cutover handoffs are immutable'); END;
CREATE TRIGGER cutover_handoffs_no_delete BEFORE DELETE ON cutover_cursor_handoffs BEGIN SELECT RAISE(ABORT, 'cutover handoffs are immutable'); END;
CREATE TRIGGER cutover_backups_no_update BEFORE UPDATE ON cutover_backup_manifests BEGIN SELECT RAISE(ABORT, 'cutover backups are immutable'); END;
CREATE TRIGGER cutover_backups_no_delete BEFORE DELETE ON cutover_backup_manifests BEGIN SELECT RAISE(ABORT, 'cutover backups are immutable'); END;
CREATE TRIGGER cutover_installations_no_update BEFORE UPDATE ON cutover_service_installations BEGIN SELECT RAISE(ABORT, 'cutover installations are immutable'); END;
CREATE TRIGGER cutover_installations_no_delete BEFORE DELETE ON cutover_service_installations BEGIN SELECT RAISE(ABORT, 'cutover installations are immutable'); END;

UPDATE schema_meta SET value = '22' WHERE key = 'version';
