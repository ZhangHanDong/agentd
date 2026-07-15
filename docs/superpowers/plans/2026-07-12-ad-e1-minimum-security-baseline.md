# AD-E1 Minimum Execution Security Baseline Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build an isolated, reviewable AD-E1 candidate that fails closed across workload identity, immutable Specify scope, fenced capabilities, secret checkout, and OCI sandbox execution without claiming AD-E1 or FSF-2 completion.

**Architecture:** Add closed security values and five independent async ports to `agentd-core`; implement reviewed cryptographic, secret, and OCI adapters in a new `agentd-security` crate; persist only workload bindings, revocation state, and capability digests in additive migration 0016; assemble one ordered enterprise admission pipeline in `agentd-bin`. Existing standalone behavior remains unchanged.

**Tech Stack:** Rust, async-trait, serde, SHA-256, OS randomness, constant-time comparison, zeroize, rustls/X.509 parsing, SQLx SQLite, typed `CommandRunner`, Tokio tests, agent-spec lifecycle.

## Candidate Constraints

- Candidate development was approved on 2026-07-15. Integration, promotion, and AD-E1/FSF-2 completion claims remain blocked on AD-E0/FSF-0 and OpenFab gates.
- Preserve Specify-owned organization, project, snapshot, RBAC, quota, and certification identities. Add no agentd-owned tenant or project source of truth.
- Persist no raw capability, secret, private key, certificate private material, or sandbox-local path.
- Keep standalone/FSF-0 routes and behavior unchanged; enterprise composition must reject open authentication and missing providers before binding a listener.
- Do not start Claude, tmux, Matrix, Specify, OpenFab, or remote services in tests. Real OCI execution is opt-in only.
- Follow RED -> GREEN -> targeted regression for each behavior slice.

---

### Task 1: Closed Core Security Contract

**Files:**
- Create: `crates/agentd-core/src/types/security.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-core/src/ports/security.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-core/tests/security.rs`

- [x] Write the two core contract selectors and verify RED on missing security APIs.
- [x] Add authenticated workload, exact authority scope, protected action/resource, capability, secret, and sandbox values with stable closed denials.
- [x] Add separate identity, authorization, capability, secret, and sandbox ports; ensure no combined bypass method exists.
- [x] Run both core selectors and existing project-authority/task-lease selectors GREEN.

### Task 2: Durable Security State and Fenced Capabilities

**Files:**
- Create: `crates/agentd-store/migrations/0016_execution_security.sql`
- Create: `crates/agentd-store/src/security_repo.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/execution_security.rs`

- [x] Write migration, exact-scope issuance, digest-only persistence, stale lease, expiry, revocation, mismatch, and denial-audit tests; verify RED.
- [x] Add workload identity bindings, revocation epoch, capability digest metadata, exact indexes, and constraints without sensitive columns.
- [x] Implement capability issue/validation with OS randomness, SHA-256 digests, constant-time comparison, current P267 incarnation, P270 lease validation, revocation epoch, and fail-closed P268 denial audit.
- [x] Run security-store selectors plus P267/P268/P270/P271 regressions GREEN.

### Task 3: Verified Workload Identity and Scoped Secrets

**Files:**
- Create: `crates/agentd-security/Cargo.toml`
- Create: `crates/agentd-security/src/lib.rs`
- Create: `crates/agentd-security/src/identity.rs`
- Create: `crates/agentd-security/src/secrets.rs`
- Create: `crates/agentd-security/tests/workload_identity.rs`
- Create: `crates/agentd-security/tests/secret_broker.rs`
- Modify: `Cargo.toml`
- Modify: `Cargo.lock`

- [ ] Write trusted/current and untrusted/expired/revoked/stale certificate tests using generated test CAs; verify RED.
- [ ] Implement chain/time/SPIFFE URI verification through reviewed TLS/X.509 libraries and exact current-incarnation binding lookup.
- [ ] Write secret admission, expiry cap, redacted Debug, serialization exclusion, wrong-scope nondisclosure, and zeroize-on-drop tests; verify RED.
- [ ] Implement the injected broker adapter without a production local-secret default and run identity/secret selectors GREEN.

### Task 4: OCI Sandbox Profile and Cleanup

**Files:**
- Create: `crates/agentd-security/src/sandbox.rs`
- Create: `crates/agentd-security/tests/sandbox.rs`
- Create: `scripts/agentd_real_security_sandbox_smoke.sh`
- Modify: `crates/agentd-bin/tests/real_execute_smoke.rs` only if shared smoke conventions require it

- [ ] Write typed launch-plan tests for immutable image digest, read-only root, explicit mounts, dropped capabilities, no-new-privileges, seccomp, resource limits, per-tenant cache, disabled network, and argv-safe execution; verify RED.
- [ ] Implement OCI plan/runner adapters through `CommandRunner`, never shell concatenation or host-worktree mounting.
- [ ] Write success/failure/cancel/timeout/recovery cleanup tests and implement idempotent teardown records.
- [ ] Add an opt-in Docker/Podman smoke whose default path starts no container; run unit and dry-run smoke selectors GREEN.

### Task 5: Ordered Enterprise Composition

**Files:**
- Create: `crates/agentd-bin/src/security.rs`
- Modify: `crates/agentd-bin/src/lib.rs`
- Modify: `crates/agentd-bin/src/cli.rs`
- Modify: `crates/agentd-bin/src/daemon.rs`
- Create: `crates/agentd-bin/tests/execution_security.rs`

- [ ] Write one recording-pipeline test that injects each stage failure and proves no later protected side effect runs except required denial audit and teardown; verify RED.
- [ ] Add explicit standalone/enterprise mode and provider selection. Enterprise startup rejects open auth or every missing identity/authorization/lease/capability/secret/sandbox/audit provider before listener creation.
- [ ] Assemble identity -> scope -> authorization -> lease/incarnation -> capability -> optional secret -> sandbox -> audit -> teardown order.
- [ ] Run enterprise selectors and all existing standalone CLI/daemon tests GREEN.

### Task 6: Repository Evidence and Candidate Verification

**Files:**
- Create: `crates/agentctl/tests/execution_security_contract.rs`
- Modify: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `specs/e2e/ad-e1-minimum-security-baseline.spec.md`
- Modify: `docs/superpowers/specs/2026-07-12-ad-e1-minimum-security-baseline-design.md`

- [ ] Add the repository artifact selector first and verify RED before roadmap/parity updates.
- [ ] Record candidate implementation evidence while keeping AD-E0, AD-E1, FSF-0, FSF-2, P272-P275, worker fleet, native runtime, Matrix cutover, and OpenFab transport incomplete.
- [ ] Run format, focused selectors, `cargo test --workspace`, workspace Clippy, `git diff --check`, and secret-pattern inspection.
- [ ] Run agent-spec lifecycle with `--ai-mode off`, explicit worktree change scope, and run log; require all 13 scenarios passing before candidate review.
- [ ] Commit reviewable slices on the isolated candidate branch. Do not merge, promote, or mark factory gates complete.
