spec: task
name: "AD-E1 minimum execution security baseline"
tags: [e2e, enterprise, security, sandbox, mtls, tenant, fencing, ad-e1, fsf-2]
---

## Intent

Build the minimum enforceable security boundary required before an
authenticated worker fleet can be designed: verified workload identity,
tenant/project authorization, lease/fencing-scoped capabilities, scoped secret
checkout, and isolated execution sandboxing. Preserve Specify authority and the
P267/P270/P271 identity, lease, and audit contracts while failing closed before
any protected side effect.

Isolated candidate implementation was approved on 2026-07-15. Integration,
promotion, and any AD-E1/FSF-2 completion claim remain blocked until AD-E0's
FSF-0 and OpenFab PRD/ADR gates are satisfied and the candidate baseline is
approved for integration.

## Decisions

- Add closed `AuthenticatedWorkload`, `ExecutionSecurityScope`,
  `ProtectedAction`, `ProtectedResource`, `CapabilityAdmission`,
  `ExecutionSandboxProfile`, and redacted secret types in `agentd-core`.
- Add `WorkloadIdentityPort`, `TenantAuthorizationPort`,
  `AttemptCapabilityPort`, `SecretBrokerPort`, and `ExecutionSandboxPort` as
  separate ports; no method may collapse identity, authorization, lease,
  capability, secret, and sandbox decisions into one call.
- Use Specify-owned `OrganizationRef`, `ProjectRef`, and
  `ProjectExecutionSnapshotRef` as tenant/project scope. Do not add an
  agentd-owned tenant or project id.
- A worker mTLS identity uses a verified SPIFFE-compatible URI and binds exactly
  one current `WorkerIncarnationId`. Trust-domain labels without certificate
  verification are not authentication.
- Attempt capabilities are 256-bit random opaque bearer values. Persist only a
  SHA-256 digest and closed metadata; compare digests in constant time and
  redact raw tokens from `Debug`, serialization, logs, and audit.
- Capability issue and validation require an exact P270 `TaskLeaseClaim`, exact
  organization/project/snapshot/action/resource scope, current worker
  incarnation, current policy revocation epoch, and an expiry no later than the
  certificate, snapshot, and lease.
- `SecretBrokerPort` accepts only a validated `secret.checkout` admission and
  returns zeroize-on-drop material whose expiry is capped by the admission.
  Enterprise startup fails closed without an explicitly selected broker.
- The first production `ExecutionSandboxProfile` is OCI-based with an immutable
  image digest, ephemeral workspace, read-only root, explicit mounts, dropped
  capabilities, no-new-privileges, seccomp, PID/memory/CPU limits, per-tenant
  cache namespace, and default-deny egress.
- Add migration `0016_execution_security.sql` for workload identity bindings,
  revocation state, and capability digests/metadata. Store no secret bytes,
  private keys, certificate private material, or sandbox-local paths.
- Reuse P268 append-only audit for stable redacted security acceptance and
  denial events. A required denial audit failure returns security unavailability
  and still performs no protected side effect.
- Enterprise production composition orders identity, scope resolution,
  authorization, lease/incarnation validation, capability admission, optional
  secret checkout, sandbox execution, audit, and cleanup. No later stage runs
  after an earlier failure.
- Existing bearer/agent-token routes remain explicit standalone/FSF-0
  compatibility only. Enterprise mode does not use `AuthConfig::open()` and
  exposes no unsecured compatibility listener.

## Boundaries

### Allowed Changes
- specs/e2e/ad-e1-minimum-security-baseline.spec.md
- docs/superpowers/specs/2026-07-12-ad-e1-minimum-security-baseline-design.md
- docs/superpowers/plans/2026-07-12-ad-e1-minimum-security-baseline.md
- docs/plans/2026-07-09-agentd-native-runtime-roadmap.md
- docs/parity/agent-chat-capability-map.md
- Cargo.toml
- Cargo.lock
- crates/agentd-core/Cargo.toml
- crates/agentd-core/src/ports/security.rs
- crates/agentd-core/src/ports/mod.rs
- crates/agentd-core/src/types/security.rs
- crates/agentd-core/src/types/mod.rs
- crates/agentd-core/tests/security.rs
- crates/agentd-security/**
- crates/agentd-store/Cargo.toml
- crates/agentd-store/migrations/0016_execution_security.sql
- crates/agentd-store/src/security_repo.rs
- crates/agentd-store/src/lib.rs
- crates/agentd-store/tests/execution_security.rs
- crates/agentd-bin/Cargo.toml
- crates/agentd-bin/src/**
- crates/agentd-bin/tests/**
- crates/agentctl/tests/execution_security_contract.rs
- scripts/agentd_real_security_sandbox_smoke.sh

### Forbidden
- Do not add an agentd-owned tenant, organization, project, repository, room,
  RBAC, quota, or certification-policy source of truth.
- Do not accept a worker trust-domain string, host name, Matrix id, agent name,
  tmux target, path, process id, or provider session as workload identity.
- Do not issue or accept a capability before exact tenant authorization and
  P270 lease/incarnation validation.
- Do not persist or log raw capability tokens, secrets, private keys,
  certificate private material, or sandbox-local paths.
- Do not default enterprise mode to open auth, audit-only agent tokens, a local
  secret map, unrestricted host execution, host worktree mounts, shared caches,
  or unrestricted egress.
- Do not hand-roll TLS, certificate-chain verification, randomness,
  constant-time comparison, or secret zeroization; use reviewed libraries.
- Do not change P200-P272 compatibility behavior in standalone/FSF-0 mode.
- Do not implement worker pull acquisition, native PTY/runtime, Matrix cutover,
  OpenFab certification transport, or legacy removal.
- Do not start Claude, tmux, Matrix, Specify, OpenFab, or remote services in
  tests. Real OCI sandbox execution remains explicitly opt-in.

## Out of Scope

- Human OIDC, browser sessions, enterprise principal provisioning, and SCIM.
- Matrix user/device/appservice principal mapping and homeserver trust policy.
- Vault, cloud KMS, cloud secret manager, SPIRE server, or Kubernetes
  deployment selection.
- Full data-classification/region/signed-image placement policy and autoscaling.
- Worker enrollment/pull/heartbeat/drain wire protocol and offline recovery.
- Native process/PTY/session ownership and provider resume.
- Retrofitting enterprise authorization into every legacy compatibility route;
  enterprise mode disables those routes until separately migrated.

## Completion Criteria

<!-- lint-ack: decision-coverage - thirteen scenarios bind every identity, authorization, capability, secret, sandbox, persistence, composition, and roadmap decision. -->
<!-- lint-ack: observable-decision-coverage - outputs are closed values/errors, durable rows/audit, ordered fake calls, filesystem/network denials, and inspected repository artifacts. -->
<!-- lint-ack: output-mode-coverage - this slice adds Rust ports, persistence, composition, and an opt-in smoke; it adds no public CLI/HTTP output format. -->
<!-- lint-ack: boundary-entry-point - scenarios bind core, store, security adapters, production composition, artifact contracts, and the opt-in smoke entry point. -->
<!-- lint-ack: bdd-rule-grouping - scenarios are grouped by identity, admission, sandbox, and composition rules. -->

Rule: identity-and-scope  verified workloads never cross tenant boundaries

Scenario: core security ports preserve separate ordered decisions
  Test:
    Package: agentd-core
    Filter: security_ports_preserve_separate_ordered_boundaries
  Level: core API contract
  Test Double: recording implementations of all five security ports
  Given closed identity authorization capability secret and sandbox requests
  When `WorkloadIdentityPort` `TenantAuthorizationPort` `AttemptCapabilityPort` `SecretBrokerPort` and `ExecutionSandboxPort` are called
  Then each request reaches only its matching port unchanged
  And no port exposes a method that bypasses an earlier decision

Scenario: security scope uses immutable Specify authority references
  Test:
    Package: agentd-core
    Filter: security_scope_uses_authority_refs_and_rejects_cross_tenant_resources
  Level: core API unit
  Test Double: none
  Given one execution scope with exact organization project snapshot policy and worker refs
  When matching and mismatched protected resources are validated
  Then matching `OrganizationRef` `ProjectRef` and `ProjectExecutionSnapshotRef` values return an authorized scope
  And organization project or snapshot mismatch returns stable closed denial reasons
  And no agentd-owned tenant or project identity exists in the security API

Scenario: workload identity accepts a current verified worker certificate
  Test:
    Package: agentd-security
    Filter: workload_identity_accepts_verified_current_worker_certificate
  Level: mTLS adapter integration
  Test Double: generated test CA certificates and temporary SQLite
  Given a trusted certificate with one SPIFFE-compatible worker incarnation subject
  And the incarnation is current in the P267 worker repository
  When the mTLS identity adapter verifies the peer
  Then it returns the exact workload role trust domain fingerprint validity and incarnation
  And no host name agent name Matrix id or trust label becomes the identity

Scenario: workload identity rejects untrusted expired revoked or stale peers
  Test:
    Package: agentd-security
    Filter: workload_identity_rejects_untrusted_expired_revoked_and_stale_peers
  Level: mTLS adapter negative integration
  Test Double: generated trusted and untrusted certificate chains plus temporary SQLite
  Given untrusted expired explicitly revoked and superseded-incarnation certificates
  When each peer is verified
  Then each returns its stable identity denial before authorization or lease access
  And no capability secret or sandbox call occurs

Rule: fenced-admission  protected actions require a current exact capability

Scenario: capability issue and validation bind exact lease and security scope
  Test:
    Package: agentd-store
    Filter: attempt_capability_binds_exact_scope_and_persists_only_digest
  Level: fenced SQLite integration
  Test Double: temporary SQLite with real P267 and P270 repositories
  Given a current worker incarnation active lease authorized tenant scope and future expiry
  When an artifact-write capability is issued and validated for the same resource
  Then the raw 256-bit token is returned once with redacted Debug
  And only its SHA-256 digest and closed metadata persist
  And validation returns an admission carrying the exact lease and fencing token

Scenario: capability rejects stale lease expiry revocation and scope mismatch
  Test:
    Package: agentd-store
    Filter: attempt_capability_rejects_stale_expired_revoked_and_wrong_scope
  Level: fenced SQLite negative integration
  Test Double: temporary SQLite with real P270 lease adapter
  Given capabilities with an old fencing token expired time explicit revocation wrong action and wrong project
  When each capability is validated
  Then each returns its stable denial without a protected side effect
  And each required denial audit identifies only safe ids scope and reason
  And an audit write failure returns security unavailable without accepting the action

Scenario: secret checkout is scoped short lived and non-observable
  Test:
    Package: agentd-security
    Filter: secret_broker_requires_checkout_admission_and_redacts_material
  Level: secret broker adapter unit
  Test Double: in-memory broker clock and recording audit
  Given one validated secret-checkout admission and one exact repository secret selector
  When the secret is checked out and dropped
  Then its expiry is no later than the capability lease certificate or snapshot
  And Debug serialization audit and persisted capability rows contain no secret value
  And the in-memory material is zeroized on drop
  When an artifact or wrong-resource admission requests the same secret
  Then checkout is denied without revealing whether the secret exists

Rule: sandbox-isolation  generated code executes only inside a closed profile

Scenario: OCI sandbox request is immutable bounded and default deny
  Test:
    Package: agentd-security
    Filter: oci_sandbox_request_is_bounded_read_only_and_default_deny
  Level: sandbox adapter unit
  Test Double: recording CommandRunner and temporary directories
  Given a validated sandbox-prepare admission and immutable image digest
  When the OCI sandbox adapter prepares an execution request
  Then root is read-only workspace is ephemeral and mounts are explicit
  And all capabilities are dropped with no-new-privileges seccomp and resource limits
  And network is disabled shared cache is absent and tenant cache namespace is exact
  And the adapter does not shell-concatenate a command

Scenario: sandbox cleanup runs after success failure cancellation and recovery
  Test:
    Package: agentd-security
    Filter: sandbox_cleanup_is_idempotent_for_all_terminal_paths
  Level: sandbox lifecycle integration
  Test Double: recording OCI runtime temporary workspace and secret mounts
  Given prepared sandboxes that succeed fail cancel time out or survive daemon interruption
  When terminal handling and recovery cleanup run repeatedly
  Then every transient secret mount and workspace is removed exactly once or reported absent
  And teardown failure is audited and leaves a recoverable teardown record

Scenario: real OCI profile denies host cross-tenant cache and network access
  Test:
    Package: agentd-security
    Filter: real_oci_sandbox_denies_host_cross_tenant_cache_and_egress
  Level: opt-in local sandbox smoke
  Test Double: no agent runtime and no external service
  Given `AGENTD_REAL_SECURITY_SANDBOX_SMOKE=1` and an available Docker or Podman runtime
  When a fixed test image probes host credentials another tenant workspace shared cache and public network
  Then every probe is denied
  And declared output is retained while workspace and secret mounts are removed
  And omission of the environment gate skips without starting a container

Rule: production-composition  enterprise mode has one ordered fail-closed path

Scenario: production security admission order stops before every failed stage
  Test:
    Package: agentd-bin
    Filter: production_security_gate_orders_checks_and_stops_on_failure
  Level: production composition contract
  Test Double: recording identity authorization lease capability secret sandbox and audit ports
  Given one scripted failure at each admission stage
  When the protected worker operation is attempted for every script
  Then calls follow identity scope authorization lease capability secret sandbox audit teardown order
  And no call after the failing stage occurs except required redacted denial audit and teardown
  And no legacy spawn backend or compatibility token path is invoked

Scenario: enterprise startup rejects missing security providers or open listener
  Test:
    Package: agentd-bin
    Filter: enterprise_security_mode_rejects_missing_providers_and_open_auth
  Level: daemon configuration contract
  Test Double: temporary configuration and no listening socket
  Given enterprise mode with each required provider missing or `AuthConfig::open()` selected
  When production composition is built
  Then startup fails with the missing closed provider name
  And no HTTP listener worker operation or sandbox starts
  When explicit standalone mode is built
  Then existing FSF-0 compatibility behavior remains unchanged

Scenario: roadmap parity and migration record baseline without claiming AD-E1 exit
  Test:
    Package: agentctl
    Filter: ad_e1_security_baseline_records_scope_and_remaining_gates
  Level: repository artifact inspection
  Test Double: repository Markdown SQL and Rust files
  Given all minimum security baseline tests pass
  When canonical roadmap parity design migration and composition are inspected
  Then they reference sandbox workload identity mTLS secret broker tenant isolation and fencing capabilities
  And migration `0016_execution_security.sql` contains no secret or private-key columns
  And P272-P275 worker fleet native runtime Matrix OpenFab cutover and scale remain unclaimed
  And AD-E0 AD-E1 FSF-0 and FSF-2 remain incomplete without their acceptance records
