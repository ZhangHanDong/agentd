-- AD-E4 signed execution evidence, OpenFab certification boundary, forge
-- admission, and Skill Hub installation history. OpenFab verdicts remain
-- externally authored; agentd stores and verifies their immutable envelopes.

CREATE TABLE trusted_evidence_signing_keys (
    key_id              TEXT PRIMARY KEY CHECK (length(trim(key_id)) BETWEEN 1 AND 256),
    signer_did          TEXT NOT NULL CHECK (signer_did GLOB 'did:key:z*'),
    signer_role         TEXT NOT NULL CHECK (signer_role IN ('builder', 'worker', 'openfab')),
    not_before          INTEGER NOT NULL,
    not_after           INTEGER NOT NULL,
    revoked_at          INTEGER,
    superseded_by       TEXT REFERENCES trusted_evidence_signing_keys(key_id) ON DELETE RESTRICT,
    registered_at       INTEGER NOT NULL,
    CHECK (not_before < not_after),
    CHECK (revoked_at IS NULL OR revoked_at >= not_before),
    UNIQUE (signer_did, signer_role, not_before)
);

CREATE INDEX idx_trusted_evidence_signing_keys_role_window
    ON trusted_evidence_signing_keys(signer_role, not_before, not_after);

CREATE TABLE signed_execution_evidence (
    envelope_id                 TEXT PRIMARY KEY CHECK (
                                    length(envelope_id) = 29
                                    AND substr(envelope_id, 1, 3) = 'ee_'
                                    AND substr(envelope_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    schema_version              INTEGER NOT NULL CHECK (schema_version = 1),
    payload_sha256              TEXT NOT NULL UNIQUE CHECK (
                                    length(payload_sha256) = 64
                                    AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    envelope_sha256             TEXT NOT NULL UNIQUE CHECK (
                                    length(envelope_sha256) = 64
                                    AND envelope_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    envelope_json               TEXT NOT NULL CHECK (json_valid(envelope_json)),
    execution_run_id            TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    execution_task_id           TEXT REFERENCES task_runs(id) ON DELETE RESTRICT,
    snapshot_authority_key      TEXT NOT NULL,
    snapshot_resource_id        TEXT NOT NULL,
    snapshot_resource_version   TEXT NOT NULL,
    snapshot_content_sha256     TEXT NOT NULL CHECK (
                                    length(snapshot_content_sha256) = 64
                                    AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    signer_key_id               TEXT NOT NULL REFERENCES trusted_evidence_signing_keys(key_id) ON DELETE RESTRICT,
    signer_did                  TEXT NOT NULL,
    signer_role                 TEXT NOT NULL CHECK (signer_role IN ('builder', 'worker')),
    signed_at                   INTEGER NOT NULL,
    stored_at                   INTEGER NOT NULL,
    UNIQUE (envelope_id, payload_sha256)
);

CREATE TABLE openfab_certification_requests (
    request_id                  TEXT PRIMARY KEY CHECK (
                                    length(request_id) = 29
                                    AND substr(request_id, 1, 3) = 'cr_'
                                    AND substr(request_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    idempotency_key             TEXT NOT NULL UNIQUE CHECK (length(trim(idempotency_key)) > 0),
    request_sha256              TEXT NOT NULL CHECK (
                                    length(request_sha256) = 64
                                    AND request_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    request_json                TEXT NOT NULL CHECK (json_valid(request_json)),
    openfab_authority_key       TEXT NOT NULL,
    envelope_id                TEXT NOT NULL REFERENCES signed_execution_evidence(envelope_id) ON DELETE RESTRICT,
    evidence_payload_sha256     TEXT NOT NULL CHECK (
                                    length(evidence_payload_sha256) = 64
                                    AND evidence_payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    snapshot_authority_key      TEXT NOT NULL,
    snapshot_resource_id        TEXT NOT NULL,
    snapshot_resource_version   TEXT NOT NULL,
    snapshot_content_sha256     TEXT NOT NULL CHECK (
                                    length(snapshot_content_sha256) = 64
                                    AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    source_commit               TEXT NOT NULL CHECK (
                                    length(source_commit) IN (40, 64)
                                    AND source_commit NOT GLOB '*[^0123456789abcdef]*'
                                ),
    subject_sha256              TEXT NOT NULL CHECK (
                                    length(subject_sha256) = 64
                                    AND subject_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    spec_sha256                 TEXT NOT NULL CHECK (
                                    length(spec_sha256) = 64
                                    AND spec_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    policy_authority_key        TEXT NOT NULL,
    policy_resource_id          TEXT NOT NULL,
    policy_resource_version     TEXT NOT NULL,
    policy_sha256               TEXT NOT NULL CHECK (
                                    length(policy_sha256) = 64
                                    AND policy_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    gate_json                   TEXT NOT NULL CHECK (json_valid(gate_json)),
    skill_packages_sha256       TEXT NOT NULL CHECK (
                                    length(skill_packages_sha256) = 64
                                    AND skill_packages_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    requested_at                INTEGER NOT NULL,
    UNIQUE (request_id, request_sha256),
    FOREIGN KEY (envelope_id, evidence_payload_sha256)
        REFERENCES signed_execution_evidence(envelope_id, payload_sha256) ON DELETE RESTRICT
);

CREATE TABLE openfab_certification_results (
    result_id                   TEXT PRIMARY KEY CHECK (
                                    length(result_id) = 29
                                    AND substr(result_id, 1, 3) = 'ce_'
                                    AND substr(result_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    request_id                  TEXT NOT NULL UNIQUE REFERENCES openfab_certification_requests(request_id) ON DELETE RESTRICT,
    result_payload_sha256       TEXT NOT NULL UNIQUE CHECK (
                                    length(result_payload_sha256) = 64
                                    AND result_payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    result_envelope_sha256      TEXT NOT NULL UNIQUE CHECK (
                                    length(result_envelope_sha256) = 64
                                    AND result_envelope_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    result_json                 TEXT NOT NULL CHECK (json_valid(result_json)),
    openfab_authority_key       TEXT NOT NULL,
    evidence_payload_sha256     TEXT NOT NULL CHECK (length(evidence_payload_sha256) = 64),
    snapshot_content_sha256     TEXT NOT NULL CHECK (length(snapshot_content_sha256) = 64),
    source_commit               TEXT NOT NULL CHECK (length(source_commit) IN (40, 64)),
    subject_sha256              TEXT NOT NULL CHECK (length(subject_sha256) = 64),
    spec_sha256                 TEXT NOT NULL CHECK (length(spec_sha256) = 64),
    policy_authority_key        TEXT NOT NULL,
    policy_resource_id          TEXT NOT NULL,
    policy_resource_version     TEXT NOT NULL,
    policy_sha256               TEXT NOT NULL CHECK (length(policy_sha256) = 64),
    skill_packages_sha256       TEXT NOT NULL CHECK (length(skill_packages_sha256) = 64),
    verdict                     TEXT NOT NULL CHECK (verdict IN ('pass', 'fail', 'revoked')),
    machine_attested            INTEGER NOT NULL CHECK (machine_attested IN (0, 1)),
    required_human_signoffs     INTEGER NOT NULL CHECK (required_human_signoffs >= 0),
    eligible_human_signoffs     INTEGER NOT NULL CHECK (eligible_human_signoffs >= required_human_signoffs),
    accepted_human_signoffs     INTEGER NOT NULL CHECK (accepted_human_signoffs >= 0),
    signer_key_id               TEXT NOT NULL REFERENCES trusted_evidence_signing_keys(key_id) ON DELETE RESTRICT,
    signer_did                  TEXT NOT NULL,
    published_at                INTEGER NOT NULL,
    revoked_at                  INTEGER,
    CHECK (accepted_human_signoffs <= eligible_human_signoffs)
);

CREATE TABLE openfab_protocol_outbox (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    event_key                   TEXT NOT NULL UNIQUE,
    event_kind                  TEXT NOT NULL CHECK (event_kind IN ('certification_request', 'certification_result')),
    aggregate_id                TEXT NOT NULL,
    payload_sha256              TEXT NOT NULL CHECK (
                                    length(payload_sha256) = 64
                                    AND payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    payload_json                TEXT NOT NULL CHECK (json_valid(payload_json)),
    created_at                  INTEGER NOT NULL,
    delivered_at               INTEGER
);

CREATE INDEX idx_openfab_protocol_outbox_pending
    ON openfab_protocol_outbox(delivered_at, sequence)
    WHERE delivered_at IS NULL;

CREATE TABLE artifact_certification_state_events (
    sequence                    INTEGER PRIMARY KEY AUTOINCREMENT,
    idempotency_key             TEXT NOT NULL UNIQUE,
    transition_sha256           TEXT NOT NULL,
    transition_json             TEXT NOT NULL CHECK (json_valid(transition_json)),
    execution_artifact_id       TEXT NOT NULL REFERENCES execution_artifacts(id) ON DELETE RESTRICT,
    previous_state              TEXT,
    next_state                  TEXT NOT NULL CHECK (
                                    next_state IN (
                                        'produced', 'delivered', 'machine_attested',
                                        'human_certified', 'released', 'revoked'
                                    )
                                ),
    certification_result_id     TEXT REFERENCES openfab_certification_results(result_id) ON DELETE RESTRICT,
    reason_code                 TEXT NOT NULL,
    observed_at                 INTEGER NOT NULL,
    CHECK (previous_state IS NULL OR previous_state IN (
        'produced', 'delivered', 'machine_attested', 'human_certified', 'released', 'revoked'
    ))
);

CREATE INDEX idx_artifact_certification_state_current
    ON artifact_certification_state_events(execution_artifact_id, sequence DESC);

CREATE TABLE forge_admissions (
    admission_id                TEXT PRIMARY KEY CHECK (
                                    length(admission_id) = 29
                                    AND substr(admission_id, 1, 3) = 'fr_'
                                    AND substr(admission_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    idempotency_key             TEXT NOT NULL UNIQUE,
    admission_sha256            TEXT NOT NULL,
    operation                   TEXT NOT NULL CHECK (operation IN ('merge', 'release')),
    snapshot_authority_key      TEXT NOT NULL,
    snapshot_resource_id        TEXT NOT NULL,
    snapshot_resource_version   TEXT NOT NULL,
    snapshot_content_sha256     TEXT NOT NULL CHECK (length(snapshot_content_sha256) = 64),
    execution_artifact_id       TEXT NOT NULL REFERENCES execution_artifacts(id) ON DELETE RESTRICT,
    source_commit               TEXT NOT NULL,
    subject_sha256              TEXT NOT NULL CHECK (length(subject_sha256) = 64),
    policy_authority_key        TEXT,
    policy_resource_id          TEXT,
    policy_resource_version     TEXT,
    policy_sha256               TEXT,
    certification_result_id     TEXT REFERENCES openfab_certification_results(result_id) ON DELETE RESTRICT,
    result_payload_sha256       TEXT,
    admitted_at                 INTEGER NOT NULL,
    CHECK (
        (certification_result_id IS NULL AND result_payload_sha256 IS NULL)
        OR (certification_result_id IS NOT NULL AND result_payload_sha256 IS NOT NULL)
    )
);

CREATE TABLE skill_package_trust_observations (
    trust_payload_sha256        TEXT PRIMARY KEY CHECK (
                                    length(trust_payload_sha256) = 64
                                    AND trust_payload_sha256 NOT GLOB '*[^0123456789abcdef]*'
                                ),
    package_authority_key       TEXT NOT NULL,
    package_resource_id         TEXT NOT NULL,
    package_resource_version    TEXT NOT NULL,
    archive_sha256              TEXT NOT NULL CHECK (length(archive_sha256) = 64),
    trust_status                TEXT NOT NULL CHECK (
                                    trust_status IN (
                                        'draft', 'in_review', 'approved', 'signed',
                                        'yanked', 'revoked', 'deprecated'
                                    )
                                ),
    trust_record_json           TEXT NOT NULL CHECK (json_valid(trust_record_json)),
    signer_key_id               TEXT NOT NULL REFERENCES trusted_evidence_signing_keys(key_id) ON DELETE RESTRICT,
    signer_did                  TEXT NOT NULL,
    status_changed_at           INTEGER NOT NULL,
    valid_until                 INTEGER NOT NULL,
    observed_at                 INTEGER NOT NULL
);

CREATE TABLE skill_installations (
    installation_id             TEXT PRIMARY KEY CHECK (
                                    length(installation_id) = 29
                                    AND substr(installation_id, 1, 3) = 'si_'
                                    AND substr(installation_id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                                ),
    admission_sha256            TEXT NOT NULL UNIQUE,
    admission_json              TEXT NOT NULL CHECK (json_valid(admission_json)),
    snapshot_authority_key      TEXT NOT NULL,
    snapshot_resource_id        TEXT NOT NULL,
    snapshot_resource_version   TEXT NOT NULL,
    snapshot_content_sha256     TEXT NOT NULL CHECK (length(snapshot_content_sha256) = 64),
    package_authority_key       TEXT NOT NULL,
    package_resource_id         TEXT NOT NULL,
    package_resource_version    TEXT NOT NULL,
    archive_sha256              TEXT NOT NULL CHECK (length(archive_sha256) = 64),
    manifest_sha256             TEXT NOT NULL CHECK (length(manifest_sha256) = 64),
    dependency_lock_sha256      TEXT NOT NULL CHECK (length(dependency_lock_sha256) = 64),
    permissions_sha256          TEXT NOT NULL CHECK (length(permissions_sha256) = 64),
    install_root_ref            TEXT NOT NULL,
    trust_status_at_install     TEXT NOT NULL CHECK (trust_status_at_install IN ('approved', 'signed')),
    trust_payload_sha256        TEXT NOT NULL REFERENCES skill_package_trust_observations(trust_payload_sha256) ON DELETE RESTRICT,
    admitted_at                 INTEGER NOT NULL,
    UNIQUE (
        snapshot_authority_key, snapshot_resource_id, snapshot_resource_version,
        package_authority_key, package_resource_id, package_resource_version,
        install_root_ref
    )
);

CREATE TRIGGER trg_trusted_evidence_keys_restricted_update
BEFORE UPDATE ON trusted_evidence_signing_keys
WHEN OLD.key_id != NEW.key_id
  OR OLD.signer_did != NEW.signer_did
  OR OLD.signer_role != NEW.signer_role
  OR OLD.not_before != NEW.not_before
  OR OLD.not_after != NEW.not_after
  OR OLD.registered_at != NEW.registered_at
  OR (OLD.revoked_at IS NOT NULL AND NEW.revoked_at IS NOT OLD.revoked_at)
  OR (OLD.superseded_by IS NOT NULL AND NEW.superseded_by IS NOT OLD.superseded_by)
BEGIN
    SELECT RAISE(ABORT, 'trusted signing key identity and lifecycle history are immutable');
END;

CREATE TRIGGER trg_trusted_evidence_keys_no_delete
BEFORE DELETE ON trusted_evidence_signing_keys
BEGIN
    SELECT RAISE(ABORT, 'trusted signing key history is immutable');
END;

CREATE TRIGGER trg_signed_execution_evidence_no_update BEFORE UPDATE ON signed_execution_evidence
BEGIN SELECT RAISE(ABORT, 'signed execution evidence is immutable'); END;
CREATE TRIGGER trg_signed_execution_evidence_no_delete BEFORE DELETE ON signed_execution_evidence
BEGIN SELECT RAISE(ABORT, 'signed execution evidence is immutable'); END;
CREATE TRIGGER trg_openfab_requests_no_update BEFORE UPDATE ON openfab_certification_requests
BEGIN SELECT RAISE(ABORT, 'OpenFab certification requests are immutable'); END;
CREATE TRIGGER trg_openfab_requests_no_delete BEFORE DELETE ON openfab_certification_requests
BEGIN SELECT RAISE(ABORT, 'OpenFab certification requests are immutable'); END;
CREATE TRIGGER trg_openfab_results_no_update BEFORE UPDATE ON openfab_certification_results
BEGIN SELECT RAISE(ABORT, 'OpenFab certification results are immutable'); END;
CREATE TRIGGER trg_openfab_results_no_delete BEFORE DELETE ON openfab_certification_results
BEGIN SELECT RAISE(ABORT, 'OpenFab certification results are immutable'); END;
CREATE TRIGGER trg_openfab_outbox_payload_no_update
BEFORE UPDATE OF event_key, event_kind, aggregate_id, payload_sha256, payload_json, created_at
ON openfab_protocol_outbox
BEGIN SELECT RAISE(ABORT, 'OpenFab protocol event payload is immutable'); END;
CREATE TRIGGER trg_openfab_outbox_no_delete BEFORE DELETE ON openfab_protocol_outbox
BEGIN SELECT RAISE(ABORT, 'OpenFab protocol event history is immutable'); END;
CREATE TRIGGER trg_certification_state_no_update BEFORE UPDATE ON artifact_certification_state_events
BEGIN SELECT RAISE(ABORT, 'certification state history is immutable'); END;
CREATE TRIGGER trg_certification_state_no_delete BEFORE DELETE ON artifact_certification_state_events
BEGIN SELECT RAISE(ABORT, 'certification state history is immutable'); END;
CREATE TRIGGER trg_forge_admissions_no_update BEFORE UPDATE ON forge_admissions
BEGIN SELECT RAISE(ABORT, 'forge admissions are immutable'); END;
CREATE TRIGGER trg_forge_admissions_no_delete BEFORE DELETE ON forge_admissions
BEGIN SELECT RAISE(ABORT, 'forge admissions are immutable'); END;
CREATE TRIGGER trg_skill_trust_no_update BEFORE UPDATE ON skill_package_trust_observations
BEGIN SELECT RAISE(ABORT, 'Skill Hub trust observations are immutable'); END;
CREATE TRIGGER trg_skill_trust_no_delete BEFORE DELETE ON skill_package_trust_observations
BEGIN SELECT RAISE(ABORT, 'Skill Hub trust observations are immutable'); END;
CREATE TRIGGER trg_skill_installations_no_update BEFORE UPDATE ON skill_installations
BEGIN SELECT RAISE(ABORT, 'skill installation history is immutable'); END;
CREATE TRIGGER trg_skill_installations_no_delete BEFORE DELETE ON skill_installations
BEGIN SELECT RAISE(ABORT, 'skill installation history is immutable'); END;

UPDATE schema_meta SET value = '20' WHERE key = 'version';
