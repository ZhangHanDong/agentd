# AD-E1 Completion Design

- Status: code-complete candidate on 2026-07-17; real/manual acceptance deferred
- Canonical roadmap: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Builds on: `docs/superpowers/specs/2026-07-12-ad-e1-minimum-security-baseline-design.md`

## Goal

Complete the remaining AD-E1 execution-security code without claiming an
AD-E1/FSF-2 exit. Real services, operator walkthroughs, release promotion, and
human sign-off are deferred to one final manual checklist after AD-E1 through
AD-E7 candidate implementation is present.

## Boundaries

Specify remains authoritative for organizations, projects, project execution
snapshots, RBAC policy versions, and revocation epochs. Agentd stores only
identity bindings, request authentication facts, device/appservice mappings,
policy observations, and enforcement evidence needed to execute a pinned
snapshot. OpenFab remains the independent certification authority.

Existing standalone/FSF-0 routes remain explicit compatibility surfaces. They
must never be exposed by an enterprise listener until the authenticated AD-E2
transport invokes the protected-operation composition.

## Components

1. `agentd-core` adds closed enterprise-principal, OIDC subject, Matrix
   identity, placement-policy, redaction, and revocation-checkpoint contracts.
2. `agentd-store` adds an additive principal lifecycle schema and repository.
   Disabling a principal or revoking a Matrix device is immediately visible to
   subsequent resolution; no bearer token, OIDC token, or device key is stored.
3. `agentd-security` verifies signed OIDC JWTs against a pinned issuer,
   audience, algorithm allowlist, and JWK set before resolving a principal.
4. Matrix resolution requires a trusted homeserver, a current user binding, a
   non-revoked device when a device is present, and an explicitly permitted
   appservice namespace for appservice senders.
5. A content redactor compiles exact secret values and policy regexes once,
   applies deterministic longest-match replacement, and enforces output-size
   bounds before transcript/log/audit persistence.
6. A placement evaluator checks data classification, region, worker trust
   domain, signed image digest, dedicated-pool requirement, egress profile, and
   tenant cache namespace before dispatch and renewal.
7. A remote secret-broker adapter uses an injected authenticated transport. It
   sends only the selector and immutable admission references and rejects
   responses whose expiry or scope exceeds the capability.
8. Revocation enforcement is one port with closed checkpoints: dispatch, lease
   renewal, artifact acceptance, delivery, and release. Every checkpoint
   compares the current authority epoch with the pinned snapshot epoch.

## Data Flow

Human/API requests first produce a verified enterprise principal. Matrix
requests additionally pass homeserver, device, and appservice resolution. The
request resolves a Specify-owned execution snapshot, then placement and
revocation checks run before the existing workload identity, tenant
authorization, lease, capability, secret, and sandbox pipeline. Protected
external side effects repeat revocation and capability validation immediately
before execution.

## Failure Model

Authentication, mapping, placement, redaction-policy loading, secret transport,
and revocation checks fail closed in enterprise mode. Stable denial codes are
audited without raw credentials or content. Disabled principals, revoked
devices, stale policy epochs, region mismatches, unsigned images, and
unavailable security providers never fall back to standalone authentication.

## Development and Final Verification

Focused unit and compile checks are development feedback and do not constitute
phase acceptance. No Claude process is started. Real OIDC, Matrix, secret
broker, OCI, worker, OpenFab, restart, failover, and human workflows are listed
in `docs/acceptance/ad-e-roadmap-manual-checklist.md` and executed together only
after the AD-E1 through AD-E7 candidate code is implemented.

## Candidate Ownership

- `agentd-core` owns closed principal, placement, and checkpoint contracts.
- `agentd-store` migration `0017` owns only non-secret OIDC/Matrix lifecycle mappings.
- `agentd-security` owns pinned OIDC verification, Matrix source resolution,
  bounded redaction, placement evaluation, remote secret scope validation, and
  Specify epoch checking.
- `agentd-bin` requires placement and policy-revocation providers in enterprise
  composition and repeats epoch checks before protected side effects.
- `docs/acceptance/ad-e-roadmap-manual-checklist.md` is the sole deferred real
  acceptance record. No candidate artifact changes AD-E1 or FSF-2 to PASS.
