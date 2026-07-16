-- AD-E3 native Matrix/Robrix gateway and per-project cutover state.
-- No raw Matrix body, runtime transcript, attachment bytes, or agent-chat
-- compatibility identity is stored here.

CREATE TABLE matrix_gateway_project_bindings (
    binding_authority_key       TEXT NOT NULL,
    binding_resource_id         TEXT NOT NULL,
    binding_resource_version    TEXT NOT NULL,
    project_authority_key       TEXT NOT NULL,
    project_resource_id         TEXT NOT NULL,
    project_resource_version    TEXT NOT NULL,
    organization_authority_key  TEXT NOT NULL,
    organization_resource_id    TEXT NOT NULL,
    organization_resource_version TEXT NOT NULL,
    snapshot_authority_key      TEXT NOT NULL,
    snapshot_resource_id        TEXT NOT NULL,
    snapshot_resource_version   TEXT NOT NULL,
    snapshot_content_sha256     TEXT NOT NULL CHECK (
                                    length(snapshot_content_sha256) = 64
                                    AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    policy_revocation_epoch     INTEGER NOT NULL CHECK (policy_revocation_epoch >= 0),
    snapshot_valid_until        INTEGER NOT NULL,
    room_id                     TEXT NOT NULL UNIQUE CHECK (length(trim(room_id)) > 0),
    mode                        TEXT NOT NULL CHECK (
                                    mode IN (
                                        'observe', 'shadow_read_only', 'canary', 'active',
                                        'draining', 'retired', 'rolled_back'
                                    )
                                ),
    sync_cursor                 TEXT NOT NULL DEFAULT '' CHECK (length(sync_cursor) <= 4096),
    previous_cursor             TEXT CHECK (previous_cursor IS NULL OR length(previous_cursor) <= 4096),
    allowed_command_classes_json TEXT NOT NULL CHECK (json_valid(allowed_command_classes_json)),
    trusted_inviters_json       TEXT NOT NULL CHECK (json_valid(trusted_inviters_json)),
    ignored_senders_json        TEXT NOT NULL CHECK (json_valid(ignored_senders_json)),
    gateway_user_id             TEXT NOT NULL CHECK (length(trim(gateway_user_id)) > 0),
    configured_at               INTEGER NOT NULL,
    updated_at                  INTEGER NOT NULL,
    PRIMARY KEY (binding_authority_key, binding_resource_id, binding_resource_version)
);

CREATE TABLE matrix_gateway_commands (
    command_id                  TEXT PRIMARY KEY CHECK (
                                    length(command_id) = 29
                                    AND substr(command_id, 1, 3) = 'mc_'
                                    AND substr(command_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    event_id                    TEXT NOT NULL UNIQUE CHECK (length(trim(event_id)) > 0),
    binding_authority_key       TEXT NOT NULL,
    binding_resource_id         TEXT NOT NULL,
    binding_resource_version    TEXT NOT NULL,
    principal_id                TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    command_class               TEXT NOT NULL CHECK (command_class IN ('execute', 'status', 'cancel')),
    gateway_mode                TEXT NOT NULL CHECK (
                                    gateway_mode IN (
                                        'observe', 'shadow_read_only', 'canary', 'active',
                                        'draining', 'retired', 'rolled_back'
                                    )
                                ),
    command_sha256              TEXT NOT NULL CHECK (
                                    length(command_sha256) = 64
                                    AND command_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    arguments_sha256            TEXT NOT NULL CHECK (
                                    length(arguments_sha256) = 64
                                    AND arguments_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    attachments_json            TEXT NOT NULL CHECK (json_valid(attachments_json)),
    disposition                 TEXT NOT NULL CHECK (
                                    disposition IN ('accepted', 'observed', 'shadowed', 'ignored', 'denied')
                                ),
    reason_code                 TEXT,
    run_id                      TEXT REFERENCES runs(id) ON DELETE RESTRICT,
    accepted_at                 INTEGER NOT NULL,
    FOREIGN KEY (binding_authority_key, binding_resource_id, binding_resource_version)
        REFERENCES matrix_gateway_project_bindings(
            binding_authority_key, binding_resource_id, binding_resource_version
        ) ON DELETE RESTRICT
);

CREATE INDEX idx_matrix_gateway_commands_binding_time
    ON matrix_gateway_commands(
        binding_authority_key, binding_resource_id, binding_resource_version,
        accepted_at DESC, command_id DESC
    );

CREATE TABLE matrix_gateway_inbox (
    event_id                    TEXT PRIMARY KEY,
    command_id                  TEXT NOT NULL UNIQUE REFERENCES matrix_gateway_commands(command_id) ON DELETE RESTRICT,
    room_id                     TEXT NOT NULL,
    sender_principal_id         TEXT NOT NULL REFERENCES enterprise_principals(id) ON DELETE RESTRICT,
    transport_sha256            TEXT NOT NULL CHECK (
                                    length(transport_sha256) = 64
                                    AND transport_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    origin_server_ts            INTEGER NOT NULL,
    processed_at                INTEGER NOT NULL
);

CREATE TABLE matrix_gateway_outbox (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    outbox_id                   TEXT NOT NULL UNIQUE CHECK (
                                    length(outbox_id) = 29
                                    AND substr(outbox_id, 1, 3) = 'mo_'
                                    AND substr(outbox_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    command_id                  TEXT NOT NULL REFERENCES matrix_gateway_commands(command_id) ON DELETE RESTRICT,
    room_id                     TEXT NOT NULL,
    event_kind                  TEXT NOT NULL CHECK (event_kind IN ('command_receipt', 'execution_summary')),
    summary                     TEXT NOT NULL CHECK (length(summary) BETWEEN 1 AND 1024),
    actionable_links_json       TEXT NOT NULL CHECK (
                                    json_valid(actionable_links_json)
                                    AND length(actionable_links_json) <= 8192
                                ),
    payload_sha256              TEXT NOT NULL CHECK (
                                    length(payload_sha256) = 64
                                    AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    created_at                  INTEGER NOT NULL,
    delivered_at                INTEGER
);

CREATE INDEX idx_matrix_gateway_outbox_pending
    ON matrix_gateway_outbox(delivered_at, sequence)
    WHERE delivered_at IS NULL;

CREATE TABLE matrix_gateway_cutover_history (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    binding_authority_key       TEXT NOT NULL,
    binding_resource_id         TEXT NOT NULL,
    binding_resource_version    TEXT NOT NULL,
    previous_mode               TEXT NOT NULL,
    next_mode                   TEXT NOT NULL,
    previous_cursor             TEXT NOT NULL,
    next_cursor                 TEXT NOT NULL,
    reason_code                 TEXT NOT NULL CHECK (length(trim(reason_code)) > 0),
    observed_at                 INTEGER NOT NULL,
    FOREIGN KEY (binding_authority_key, binding_resource_id, binding_resource_version)
        REFERENCES matrix_gateway_project_bindings(
            binding_authority_key, binding_resource_id, binding_resource_version
        ) ON DELETE RESTRICT
);

CREATE TABLE matrix_gateway_state_mappings (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    binding_authority_key       TEXT NOT NULL,
    binding_resource_id         TEXT NOT NULL,
    binding_resource_version    TEXT NOT NULL,
    mapping_kind                TEXT NOT NULL CHECK (
                                    mapping_kind IN (
                                        'project', 'room', 'principal', 'task',
                                        'message', 'cursor', 'run'
                                    )
                                ),
    legacy_ref_sha256           TEXT NOT NULL CHECK (
                                    length(legacy_ref_sha256) = 64
                                    AND legacy_ref_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    canonical_ref               TEXT NOT NULL CHECK (length(canonical_ref) BETWEEN 1 AND 1024),
    in_flight                   INTEGER NOT NULL CHECK (in_flight IN (0, 1)),
    observed_at                 INTEGER NOT NULL,
    UNIQUE (
        binding_authority_key, binding_resource_id, binding_resource_version,
        mapping_kind, legacy_ref_sha256
    ),
    FOREIGN KEY (binding_authority_key, binding_resource_id, binding_resource_version)
        REFERENCES matrix_gateway_project_bindings(
            binding_authority_key, binding_resource_id, binding_resource_version
        ) ON DELETE RESTRICT
);

CREATE INDEX idx_matrix_gateway_state_mappings_binding
    ON matrix_gateway_state_mappings(
        binding_authority_key, binding_resource_id, binding_resource_version,
        mapping_kind, sequence
    );

CREATE TRIGGER trg_matrix_gateway_commands_no_update
BEFORE UPDATE ON matrix_gateway_commands
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway command ledger is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_commands_no_delete
BEFORE DELETE ON matrix_gateway_commands
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway command ledger is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_inbox_no_update
BEFORE UPDATE ON matrix_gateway_inbox
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway inbox is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_inbox_no_delete
BEFORE DELETE ON matrix_gateway_inbox
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway inbox is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_outbox_no_update
BEFORE UPDATE OF outbox_id, command_id, room_id, event_kind, summary,
    actionable_links_json, payload_sha256, created_at
ON matrix_gateway_outbox
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway outbox payload is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_outbox_no_delete
BEFORE DELETE ON matrix_gateway_outbox
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway outbox history is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_cutover_no_update
BEFORE UPDATE ON matrix_gateway_cutover_history
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway cutover history is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_cutover_no_delete
BEFORE DELETE ON matrix_gateway_cutover_history
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway cutover history is immutable');
END;

CREATE TRIGGER trg_matrix_gateway_state_mappings_no_update
BEFORE UPDATE ON matrix_gateway_state_mappings
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway state mappings are immutable');
END;

CREATE TRIGGER trg_matrix_gateway_state_mappings_no_delete
BEFORE DELETE ON matrix_gateway_state_mappings
BEGIN
    SELECT RAISE(ABORT, 'Matrix gateway state mappings are immutable');
END;

UPDATE schema_meta SET value = '19' WHERE key = 'version';
