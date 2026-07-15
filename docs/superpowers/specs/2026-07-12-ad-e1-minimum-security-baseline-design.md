# AD-E1 Minimum Execution Security Baseline Design

- Status: isolated candidate implementation approved 2026-07-15; integration and promotion blocked on AD-E0 gates
- Date: 2026-07-12
- Canonical roadmap: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Factory mapping: AD-E1 / FSF-2 minimum worker-side prerequisite

## 1. Purpose

This design defines the minimum security boundary that must exist before the
historical P278 worker-fleet scope can be re-specified under AD-E2. It covers
five inseparable properties: execution sandboxing, workload identity with mTLS,
a scoped secret broker, tenant/project isolation, and short-lived capabilities
bound to the current task lease and fencing token.

This is not the complete AD-E1/FSF-2 exit gate. Human OIDC, Matrix principal
mapping, production secret-manager products, and every enterprise policy field
remain later AD-E1 work. Completing this baseline authorizes design of AD-E2; it
does not by itself authorize production deployment or P263-P271 integration.

## 2. Chosen Approach

Use composable core ports with an `agentd-security` adapter crate and one strict
production admission pipeline. The security decision is not embedded in
`agentd-surface::AuthConfig`, the scheduler, the tmux backend, or a vendor SDK.

Alternatives rejected:

- Extending bearer and per-agent tokens cannot bind identity to a tenant,
  certificate, immutable project snapshot, lease, or fencing token.
- A single security service trait would hide ordering and make negative-path
  tests unable to prove which check failed before a side effect.
- Selecting Vault, SPIRE, Kubernetes, or one cloud KMS now would couple the core
  contract to deployment products before the factory chooses them.
- Self-contained hand-rolled signed tokens are unnecessary for the first
  control-plane-mediated slice. Opaque random tokens with server-side state make
  revocation and exact-scope validation explicit.

## 3. Security Invariants

1. Enterprise execution fails closed if workload identity, tenant authorization,
   lease validation, capability admission, secret brokering, or sandbox setup is
   unavailable.
2. Specify remains tenant/project authority. Security scopes use immutable
   `OrganizationRef`, `ProjectRef`, and `ProjectExecutionSnapshotRef`; agentd
   does not create an independent tenant or project identity.
3. A worker identity names exactly one current `WorkerIncarnationId`. Certificate
   trust-domain labels are routing inputs, not proof without successful mTLS
   verification and current-incarnation validation.
4. Every protected action is bound to an exact `TaskLeaseClaim`, organization,
   project, snapshot, action, resource selector, issue time, and expiry time.
5. Capability validity is no longer than the workload certificate, project
   snapshot, and current lease validity windows.
6. A capability is rejected after lease supersession, stale fencing, terminal
   lease state, worker-incarnation replacement, expiry, or explicit revocation.
7. Secret bytes are never serialized, persisted in capability rows, formatted
   through `Debug`, written to audit payloads, or placed in daemon-wide worker
   configuration.
8. Enterprise sandbox profiles use an ephemeral workspace, read-only root,
   explicit mounts, dropped capabilities, process/resource limits, no-new-
   privileges, and default-deny network egress.
9. Cleanup is idempotent and removes the workspace and transient secret mounts
   after success, failure, cancellation, timeout, or daemon recovery.
10. A denial is audited with stable reason codes and identifiers, but never with
    bearer tokens, certificate private material, or secret values.

## 4. Core Model

### 4.1 Workload Identity

`AuthenticatedWorkload` contains a verified SPIFFE-compatible URI, workload
role, trust domain, certificate fingerprint, validity window, and an optional
worker/incarnation binding. The mTLS adapter constructs this value only after
chain, hostname/SAN, validity, and configured trust-root verification.

The minimum closed roles are `ControlPlane`, `Gateway`, and `Worker`. Worker
requests must carry both `WorkerId` and `WorkerIncarnationId`, and the
incarnation must still be current in the P267 repository.

### 4.2 Execution Security Scope

`ExecutionSecurityScope` contains:

- authority key;
- organization, project, and execution-snapshot references;
- RBAC policy version reference;
- worker incarnation;
- task lease claim;
- sandbox profile and egress profile ids;
- policy revocation epoch and validity window.

`TenantAuthorizationPort` accepts an authenticated workload, one closed
`ProtectedAction`, one closed `ProtectedResource`, and this scope. Exact
organization/project/snapshot mismatch is a denial, never a fallback.

### 4.3 Fencing-Scoped Capability

`AttemptCapabilityPort` issues a 256-bit random opaque token and persists only
its SHA-256 digest plus the closed scope. The raw token is returned once and is
redacted from `Debug`. Validation performs constant-time digest comparison,
checks action/resource/scope and time, calls `TaskLeasePort::validate_claim`,
and checks current worker incarnation and policy revocation epoch.

The first action set is:

- `sandbox.prepare` and `sandbox.execute`;
- `secret.checkout`;
- `artifact.read` and `artifact.write`;
- `forge.read` and `forge.write`;
- `tool.high_risk`.

Capabilities are control-plane mediated. Protected adapters validate
immediately before each external side effect and propagate the lease/fencing
identity as idempotency/admission metadata where the external protocol allows.

### 4.4 Secret Broker

`SecretBrokerPort::checkout` consumes an already validated
`CapabilityAdmission` for `secret.checkout` and an exact secret selector. It
returns `SecretLease`, whose expiry is capped by the capability, certificate,
snapshot, and task lease. `SecretMaterial` uses zeroize-on-drop storage and a
redacted `Debug` implementation.

The baseline provides an injected in-memory/local adapter for deterministic
standalone tests. Enterprise mode fails startup without an explicitly selected
broker adapter. Vault, cloud KMS, and repository-app installation are later
adapters behind the same port.

### 4.5 Execution Sandbox

`ExecutionSandboxPort` consumes a validated `sandbox.prepare` admission and a
closed `ExecutionSandboxProfile`. The first production profile is OCI-based:

- immutable image selected by digest;
- read-only root filesystem;
- tmpfs/ephemeral workspace;
- no host worktree, credential, socket, or home-directory mounts;
- explicit read-only input and bounded output mounts;
- all Linux capabilities dropped, `no-new-privileges`, seccomp, PID/memory/CPU
  limits;
- network disabled unless the named egress profile contains an allowlist;
- per-tenant cache namespace, with shared caches disabled by default.

Core code does not shell-build an OCI command. The adapter generates a typed
launch request and executes it through the existing `CommandRunner` boundary.
Unit tests use a recording runner; an opt-in smoke can use Docker or Podman but
must not start Claude, tmux, Matrix, Specify, or OpenFab.

## 5. Admission Flow

For every protected worker operation, production composition executes this
order:

1. terminate and verify mTLS, producing `AuthenticatedWorkload`;
2. resolve the immutable execution security scope;
3. verify tenant/project/action/resource authorization;
4. validate the current P270 lease claim and P267 worker incarnation;
5. issue or validate the exact fencing-scoped capability;
6. check out only requested secrets, if required;
7. prepare and execute inside the selected sandbox;
8. append acceptance or stable redacted denial audit;
9. clean up transient secrets and sandbox resources.

No later stage runs when an earlier stage fails. Existing open/local
compatibility routes remain available only in explicit standalone/FSF-0 mode.
Enterprise mode must not expose a second unsecured listener or silently select
`AuthConfig::open()`.

## 6. Persistence

Implementation uses additive migration `0016_execution_security.sql` after
P270's `0015` migration. It adds:

- workload identity bindings and revocation state;
- opaque attempt-capability metadata and token digests;
- indexes for current worker identity, expiry/revocation reaping, and exact
  lease/fencing lookup.

No secret bytes, private keys, certificate private material, or sandbox-local
paths are stored. Security decisions reuse P268's append-only audit events.
Capability issuance, revocation, and rejection have stable idempotency keys.

## 7. Error Model

Closed denial reasons include `identity_untrusted`, `identity_expired`,
`identity_revoked`, `incarnation_stale`, `tenant_mismatch`, `project_mismatch`,
`snapshot_mismatch`, `action_denied`, `resource_denied`, `capability_expired`,
`capability_revoked`, `capability_scope_mismatch`, `lease_rejected`,
`secret_unavailable`, `sandbox_profile_denied`, `sandbox_start_failed`, and
`sandbox_cleanup_failed`.

Authentication failures do not disclose whether a tenant, project, worker,
lease, capability, or secret exists. Operator diagnostics can reveal the stable
reason only through an independently authorized audit path.

## 8. Implementation Slices

1. Security types, closed actions/resources, authorization port, and tests.
2. Workload identity bindings, mTLS verifier adapter, and revocation tests.
3. Durable opaque capabilities, lease/fencing validation, redacted audit, and
   secret broker port.
4. OCI sandbox profile/adapter, cleanup recovery, and isolation tests.
5. Enterprise composition gate and end-to-end admission-order tests.

Each slice is independently reviewable. No worker-fleet acquisition protocol or
native runtime implementation starts inside these slices.

## 9. Verification and Gate Meaning

The umbrella agent-spec binds core, store, security-adapter, composition, and
opt-in OCI smoke evidence. Tests use fakes, temporary SQLite, and recording
command runners. No test invokes Claude. Codex is allowed only for an explicit
real smoke requested separately.

Passing this baseline means the AD-E2 worker protocol may be designed against a
real security boundary. It does not mean AD-E0, AD-E1, FSF-0, or FSF-2 has
passed, and it does not authorize main integration without their acceptance
records and human sign-off.

The 2026-07-15 authorization permits implementation and verification only on an
isolated candidate branch. It does not relax any factory gate, acceptance
record, immutable-candidate, or human-sign-off requirement.

## 10. Deferred AD-E1 Work

- Human OIDC and enterprise-principal lifecycle.
- Matrix user/device/appservice principal mapping and homeserver trust policy.
- Production Vault/KMS/cloud secret-broker adapters and key rotation.
- Full data-classification/region/signed-image placement policy.
- Transcript/log content redaction beyond secret-value exclusion.
- Cross-tenant enforcement over every legacy FSF-0 compatibility route; those
  routes remain disabled in enterprise mode until separately migrated.
