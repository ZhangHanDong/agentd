-- AD-E1 candidate / workload bindings, policy epochs, and digest-only
-- attempt capabilities. No bearer token, secret, private key, certificate
-- private material, or sandbox-local path is stored here.

CREATE TABLE workload_identity_bindings (
    certificate_sha256     TEXT PRIMARY KEY
                           CHECK (
                               length(certificate_sha256) = 64
                               AND certificate_sha256 NOT GLOB '*[^0123456789abcdef]*'
                           ),
    spiffe_uri             TEXT NOT NULL UNIQUE CHECK (length(trim(spiffe_uri)) > 0),
    role                   TEXT NOT NULL CHECK (role IN ('control_plane', 'gateway', 'worker')),
    trust_domain           TEXT NOT NULL CHECK (length(trim(trust_domain)) > 0),
    worker_id              TEXT REFERENCES workers(id) ON DELETE RESTRICT,
    worker_incarnation_id  TEXT REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    not_before             INTEGER NOT NULL,
    not_after              INTEGER NOT NULL CHECK (not_after > not_before),
    revoked_at             INTEGER,
    revocation_reason      TEXT,
    created_at             INTEGER NOT NULL,
    CHECK (
        (role = 'worker' AND worker_id IS NOT NULL AND worker_incarnation_id IS NOT NULL)
        OR (role <> 'worker' AND worker_id IS NULL AND worker_incarnation_id IS NULL)
    ),
    CHECK (
        (revoked_at IS NULL AND revocation_reason IS NULL)
        OR (revoked_at IS NOT NULL AND length(trim(revocation_reason)) > 0)
    )
);

CREATE INDEX idx_workload_identity_bindings_worker
    ON workload_identity_bindings(worker_incarnation_id, revoked_at, not_after)
    WHERE worker_incarnation_id IS NOT NULL;

CREATE TABLE execution_security_policy_epochs (
    authority_key          TEXT NOT NULL CHECK (length(trim(authority_key)) > 0),
    organization_id        TEXT NOT NULL CHECK (length(trim(organization_id)) > 0),
    organization_version   TEXT NOT NULL CHECK (length(trim(organization_version)) > 0),
    project_id             TEXT NOT NULL CHECK (length(trim(project_id)) > 0),
    project_version        TEXT NOT NULL CHECK (length(trim(project_version)) > 0),
    snapshot_id            TEXT NOT NULL CHECK (length(trim(snapshot_id)) > 0),
    snapshot_version       TEXT NOT NULL CHECK (length(trim(snapshot_version)) > 0),
    current_epoch          INTEGER NOT NULL CHECK (current_epoch >= 0),
    updated_at             INTEGER NOT NULL,
    PRIMARY KEY (
        authority_key,
        organization_id,
        organization_version,
        project_id,
        project_version,
        snapshot_id,
        snapshot_version
    )
);

CREATE TRIGGER trg_execution_security_policy_epoch_monotonic
BEFORE UPDATE OF current_epoch ON execution_security_policy_epochs
WHEN NEW.current_epoch < OLD.current_epoch
BEGIN
    SELECT RAISE(ABORT, 'execution security policy epoch cannot decrease');
END;

CREATE TABLE attempt_capabilities (
    id                         TEXT PRIMARY KEY
                               CHECK (
                                   length(id) = 29
                                   AND substr(id, 1, 3) = 'cp_'
                                   AND substr(id, 4) NOT GLOB '*[^0123456789ABCDEFGHJKMNPQRSTVWXYZ]*'
                               ),
    token_sha256               TEXT NOT NULL UNIQUE
                               CHECK (
                                   length(token_sha256) = 64
                                   AND token_sha256 NOT GLOB '*[^0123456789abcdef]*'
                               ),
    spiffe_uri                 TEXT NOT NULL CHECK (length(trim(spiffe_uri)) > 0),
    workload_role              TEXT NOT NULL CHECK (
                                   workload_role IN ('control_plane', 'gateway', 'worker')
                               ),
    trust_domain               TEXT NOT NULL CHECK (length(trim(trust_domain)) > 0),
    certificate_sha256         TEXT NOT NULL
                               CHECK (
                                   length(certificate_sha256) = 64
                                   AND certificate_sha256 NOT GLOB '*[^0123456789abcdef]*'
                               ),
    certificate_not_before     INTEGER NOT NULL,
    certificate_not_after      INTEGER NOT NULL,
    worker_id                  TEXT NOT NULL REFERENCES workers(id) ON DELETE RESTRICT,
    worker_incarnation_id      TEXT NOT NULL REFERENCES worker_incarnations(id) ON DELETE RESTRICT,
    execution_task_id          TEXT NOT NULL REFERENCES task_runs(id) ON DELETE RESTRICT,
    lease_id                   TEXT NOT NULL REFERENCES execution_task_leases(id) ON DELETE RESTRICT,
    fencing_token              INTEGER NOT NULL CHECK (fencing_token > 0),
    authority_key              TEXT NOT NULL CHECK (length(trim(authority_key)) > 0),
    organization_id            TEXT NOT NULL CHECK (length(trim(organization_id)) > 0),
    organization_version       TEXT NOT NULL CHECK (length(trim(organization_version)) > 0),
    project_id                 TEXT NOT NULL CHECK (length(trim(project_id)) > 0),
    project_version            TEXT NOT NULL CHECK (length(trim(project_version)) > 0),
    snapshot_id                TEXT NOT NULL CHECK (length(trim(snapshot_id)) > 0),
    snapshot_version           TEXT NOT NULL CHECK (length(trim(snapshot_version)) > 0),
    rbac_policy_id             TEXT NOT NULL CHECK (length(trim(rbac_policy_id)) > 0),
    rbac_policy_version        TEXT NOT NULL CHECK (length(trim(rbac_policy_version)) > 0),
    sandbox_profile_id         TEXT NOT NULL CHECK (length(trim(sandbox_profile_id)) > 0),
    egress_profile_id          TEXT NOT NULL CHECK (length(trim(egress_profile_id)) > 0),
    policy_revocation_epoch    INTEGER NOT NULL CHECK (policy_revocation_epoch >= 0),
    scope_valid_until          INTEGER NOT NULL,
    action                     TEXT NOT NULL CHECK (
                                   action IN (
                                       'sandbox.prepare', 'sandbox.execute', 'secret.checkout',
                                       'artifact.read', 'artifact.write', 'forge.read',
                                       'forge.write', 'tool.high_risk'
                                   )
                               ),
    resource_json              TEXT NOT NULL CHECK (json_valid(resource_json)),
    execution_run_id           TEXT NOT NULL REFERENCES runs(id) ON DELETE RESTRICT,
    snapshot_content_sha256    TEXT NOT NULL
                               CHECK (
                                   length(snapshot_content_sha256) = 64
                                   AND snapshot_content_sha256 NOT GLOB '*[^0123456789abcdef]*'
                               ),
    target_repository_id       TEXT NOT NULL CHECK (length(trim(target_repository_id)) > 0),
    target_base_commit         TEXT NOT NULL CHECK (length(trim(target_base_commit)) > 0),
    issued_at                  INTEGER NOT NULL,
    expires_at                 INTEGER NOT NULL CHECK (expires_at > issued_at),
    revoked_at                 INTEGER,
    revocation_reason          TEXT,
    CHECK (certificate_not_after > certificate_not_before),
    CHECK (expires_at <= certificate_not_after),
    CHECK (expires_at <= scope_valid_until),
    CHECK (
        (revoked_at IS NULL AND revocation_reason IS NULL)
        OR (revoked_at IS NOT NULL AND length(trim(revocation_reason)) > 0)
    )
);

CREATE INDEX idx_attempt_capabilities_lease_scope
    ON attempt_capabilities(
        lease_id,
        fencing_token,
        worker_incarnation_id,
        action,
        expires_at
    );

CREATE INDEX idx_attempt_capabilities_policy_scope
    ON attempt_capabilities(
        authority_key,
        organization_id,
        project_id,
        snapshot_id,
        policy_revocation_epoch,
        expires_at
    );

CREATE INDEX idx_attempt_capabilities_reap
    ON attempt_capabilities(expires_at, revoked_at, id);

CREATE TRIGGER trg_attempt_capabilities_identity_immutable
BEFORE UPDATE OF
    id, token_sha256, spiffe_uri, workload_role, trust_domain,
    certificate_sha256, certificate_not_before, certificate_not_after,
    worker_id, worker_incarnation_id, execution_task_id, lease_id,
    fencing_token, authority_key, organization_id, organization_version,
    project_id, project_version, snapshot_id, snapshot_version,
    rbac_policy_id, rbac_policy_version, sandbox_profile_id,
    egress_profile_id, policy_revocation_epoch, scope_valid_until,
    action, resource_json, execution_run_id, snapshot_content_sha256,
    target_repository_id, target_base_commit, issued_at, expires_at
ON attempt_capabilities
BEGIN
    SELECT RAISE(ABORT, 'attempt capability identity and scope are immutable');
END;

CREATE TRIGGER trg_attempt_capabilities_no_delete
BEFORE DELETE ON attempt_capabilities
BEGIN
    SELECT RAISE(ABORT, 'attempt capability history is immutable');
END;

UPDATE schema_meta SET value = '16' WHERE key = 'version';
