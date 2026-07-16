# AD-E1 Completion Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Complete the remaining AD-E1 code for enterprise principals, OIDC and Matrix identity, content redaction, placement, remote secrets, and policy-revocation checkpoints while deferring real/manual acceptance.

**Architecture:** Extend the existing closed security types and ports, persist only non-secret lifecycle state in additive SQLite migrations, and implement product-neutral adapters in `agentd-security`. AD-E2 will consume these ports from its authenticated worker transport; standalone behavior stays unchanged.

**Tech Stack:** Rust 2024, async-trait, serde, SQLx SQLite, jsonwebtoken/JWK, regex-automata, existing rustls/X.509, existing audit and lease ports.

## Global Constraints

- Specify owns organization, project, snapshot, RBAC, quota, and revocation truth.
- Store no bearer/OIDC token, secret bytes, device keys, private keys, or raw transcript content in identity/security tables.
- Enterprise authentication and authorization fail closed; no fallback to standalone tokens.
- Tests must not invoke Claude. Codex is reserved for the final explicit real smoke.
- Focused automated checks are development feedback; all real/manual acceptance is deferred to `docs/acceptance/ad-e-roadmap-manual-checklist.md`.

---

### Task 1: Enterprise Principal and Policy Contracts

**Files:**
- Create: `crates/agentd-core/src/types/principal.rs`
- Create: `crates/agentd-core/src/ports/principal.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-core/tests/principal_security.rs`

**Interfaces:**
- Produces: `EnterprisePrincipalId`, `EnterprisePrincipal`, `PrincipalStatus`, `PrincipalKind`, `OidcIdentity`, `MatrixIdentity`, `EnterpriseRequestIdentity`.
- Produces: `EnterprisePrincipalPort::resolve_oidc`, `resolve_matrix`, and `get_principal`.
- Produces: `PlacementPolicy`, `PlacementCandidate`, `SecurityCheckpoint`, and `PolicyRevocationPort`.

- [x] **Step 1:** Add compile-time contract tests proving disabled principals, revoked Matrix devices, untrusted homeservers, and stale revocation epochs are closed denials.
- [x] **Step 2:** Run `cargo test -p agentd-core --test principal_security` and require RED because the contracts do not exist.
- [x] **Step 3:** Add the closed types, validation constructors, denial variants, and async ports without I/O or product SDK types.
- [x] **Step 4:** Run the focused core selector GREEN and commit `feat(security): define enterprise principal contracts`.

### Task 2: Durable Principal Lifecycle

**Files:**
- Create: `crates/agentd-store/migrations/0017_enterprise_principals.sql`
- Create: `crates/agentd-store/src/principal_repo.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/enterprise_principals.rs`

**Interfaces:**
- Consumes: Task 1 principal and Matrix types.
- Produces: `SqliteEnterprisePrincipalRepository` implementing `EnterprisePrincipalPort` plus explicit `upsert_principal`, `disable_principal`, `bind_oidc_subject`, `bind_matrix_user`, `bind_matrix_device`, and `revoke_matrix_device` lifecycle methods.

- [x] **Step 1:** Add migration and repository tests for current resolution, duplicate issuer/subject rejection, disablement, device revocation, homeserver trust, appservice namespace, idempotent updates, and absence of secret columns.
- [x] **Step 2:** Run `cargo test -p agentd-store --test enterprise_principals` and require RED.
- [x] **Step 3:** Implement additive schema and repository with transactional lifecycle updates and stable audit references.
- [x] **Step 4:** Run the focused store selector GREEN and commit `feat(store): persist enterprise principal lifecycle`.

### Task 3: OIDC and Matrix Authentication Adapters

**Files:**
- Create: `crates/agentd-security/src/oidc.rs`
- Create: `crates/agentd-security/src/matrix_principal.rs`
- Modify: `crates/agentd-security/src/lib.rs`
- Modify: `crates/agentd-security/Cargo.toml`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `crates/agentd-security/tests/oidc.rs`
- Create: `crates/agentd-security/tests/matrix_principal.rs`

**Interfaces:**
- Produces: `OidcAuthenticator<R>` over an injected `EnterprisePrincipalPort` and pinned `OidcProviderConfig`/JWK set.
- Produces: `MatrixPrincipalResolver<R>` enforcing homeserver, user, device, and appservice mapping.

- [ ] **Step 1:** Add signed-token tests covering issuer, audience, expiry, not-before, algorithm, kid, subject mapping, disabled principal, and token-redacted errors.
- [ ] **Step 2:** Add Matrix tests covering trusted/untrusted homeserver, active/disabled user, current/revoked device, and allowed/foreign appservice namespace.
- [ ] **Step 3:** Implement verification with `jsonwebtoken` and repository-backed resolution; never parse an unverified claim into an authenticated identity.
- [ ] **Step 4:** Run both focused selectors GREEN and commit `feat(security): authenticate oidc and matrix principals`.

### Task 4: Content Redaction and Placement Admission

**Files:**
- Create: `crates/agentd-security/src/redaction.rs`
- Create: `crates/agentd-security/src/placement.rs`
- Modify: `crates/agentd-security/src/lib.rs`
- Modify: `crates/agentd-security/Cargo.toml`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`
- Create: `crates/agentd-security/tests/redaction.rs`
- Create: `crates/agentd-security/tests/placement.rs`

**Interfaces:**
- Produces: `ContentRedactor::compile` and `redact` with exact-value and policy-pattern rules.
- Produces: `PlacementPolicyEvaluator::evaluate(&PlacementPolicy, &PlacementCandidate)` returning a closed `PlacementAdmission`.

- [ ] **Step 1:** Add redaction tests for overlap, regex policy, binary/UTF-8 handling, output bounds, deterministic replacement, and no raw secret in Debug/errors.
- [ ] **Step 2:** Add placement tests for classification, region, trust domain, signed digest, dedicated pool, egress, and tenant cache isolation.
- [ ] **Step 3:** Implement bounded redaction with `regex-automata` and exact longest-match ordering, then implement pure placement evaluation.
- [ ] **Step 4:** Run focused selectors GREEN and commit `feat(security): enforce redaction and placement policy`.

### Task 5: Remote Secret Broker and Revocation Checkpoints

**Files:**
- Create: `crates/agentd-security/src/remote_secrets.rs`
- Create: `crates/agentd-security/src/revocation.rs`
- Modify: `crates/agentd-security/src/lib.rs`
- Create: `crates/agentd-security/tests/remote_secrets.rs`
- Create: `crates/agentd-security/tests/revocation.rs`
- Modify: `crates/agentd-bin/src/security.rs`
- Modify: `crates/agentd-bin/tests/execution_security.rs`

**Interfaces:**
- Produces: `SecretBrokerTransport` and `RemoteSecretBroker<T>` implementing `SecretBrokerPort`.
- Produces: `AuthorityRevocationChecker<A>` implementing `PolicyRevocationPort` for all closed checkpoints.
- Extends: enterprise operation composition with placement and checkpoint checks before protected side effects.

- [ ] **Step 1:** Add fake-transport tests proving exact selector/scope requests, bounded expiry, redacted failures, timeout/unavailability denial, and no secret persistence.
- [ ] **Step 2:** Add dispatch/renewal/artifact/delivery/release tests for equal, advanced, unavailable, and malformed authority epochs.
- [ ] **Step 3:** Implement adapters and insert current checkpoint validation before every protected external side effect.
- [ ] **Step 4:** Run focused selectors GREEN and commit `feat(security): enforce remote secrets and revocation checkpoints`.

### Task 6: Candidate Evidence and Deferred Manual Checklist

**Files:**
- Create: `docs/acceptance/ad-e-roadmap-manual-checklist.md`
- Create: `crates/agentctl/tests/ad_e1_completion_contract.rs`
- Modify: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `specs/e2e/ad-e1-minimum-security-baseline.spec.md`
- Modify: `docs/superpowers/specs/2026-07-17-ad-e1-completion-design.md`

**Interfaces:**
- Produces: repository-level proof that every AD-E1 roadmap work item has code ownership while keeping AD-E1/FSF-2 unaccepted until final manual verification.
- Produces: one cumulative manual checklist carried forward through AD-E7.

- [ ] **Step 1:** Add a repository artifact test binding every new type, port, migration, adapter, and deferred real/manual scenario.
- [ ] **Step 2:** Update roadmap/parity/spec status to code-complete candidate without setting an exit gate to PASS.
- [ ] **Step 3:** Append exact OIDC, Matrix, secret broker, OCI, cross-tenant, revocation, and placement manual commands/evidence fields to the cumulative checklist.
- [ ] **Step 4:** Run automated workspace feedback, record failures without stopping later phase coding, and commit `docs(security): record complete ad-e1 candidate`.
