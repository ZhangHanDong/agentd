# P268 Artifact Audit Store Model Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add immutable enterprise execution-artifact metadata, explicit legacy/OpenFab references, and ordered idempotent execution-audit persistence.

**Architecture:** Extend core with artifact/audit ids. Add migration 0014 without changing compatibility tables, then implement separate artifact-index and audit-log repositories with parent-chain validation and database immutability triggers.

**Tech Stack:** Rust, SQLx runtime queries, SQLite constraints/triggers/transactions, Tokio integration tests, agent-spec lifecycle.

## Global Constraints

- Preserve every existing P0/p200-p267 table, row, API, and command behavior.
- Keep Specify project authority and OpenFab certification authority external.
- Add no dependency, network, object-storage, HTTP, CLI, MCP, Matrix, or daemon behavior.
- Tests use temporary/in-memory SQLite and fake records only; start no agent/runtime/external service.
- The shared dirty worktree is not committed by this slice executor.

---

### Task 1: Artifact and Audit Identity Contracts

**Files:**
- Modify: `crates/agentd-core/src/types/ids.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-core/tests/enterprise_artifact_identity.rs`

- [x] **Step 1: Write and run the failing identity test**

Run: `cargo test -p agentd-core --test enterprise_artifact_identity enterprise_artifact_and_audit_ids_are_distinct`
Expected: RED because the P268 id types do not exist.

- [x] **Step 2: Add id newtypes and verify GREEN**

Implement `ExecutionArtifactId` (`ar_`) and `AuditEventId` (`ae_`) through the existing id macro, then rerun the selector.

### Task 2: Additive Migration and Backcompat

**Files:**
- Create: `crates/agentd-store/migrations/0014_enterprise_artifact_audit.sql`
- Modify: `crates/agentd-store/tests/migration.rs`
- Modify: `crates/agentd-store/tests/migration_backcompat.rs`

- [x] **Step 1: Add migration/backcompat tests and verify RED**

Run the two P268 migration selectors before creating migration 0014.

- [x] **Step 2: Implement migration 0014 and verify GREEN**

Create four additive tables, indexes, immutability triggers, and schema version 14. Preserve and byte-compare representative legacy artifact/event rows.

### Task 3: Enterprise Artifact Repository

**Files:**
- Create: `crates/agentd-store/src/execution_artifact_repo.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/enterprise_execution_artifacts.rs`

- [x] **Step 1: Add artifact happy/error tests and verify RED**

Cover immutable metadata, parent-chain validation, malformed input, explicit legacy mapping, OpenFab refs, retries, remaps, and SQL triggers.

- [x] **Step 2: Implement the minimal repository and verify GREEN**

Use transactions for parent validation plus insert. Keep certification references append-only and never update an artifact to attach certification.

### Task 4: Enterprise Audit Repository

**Files:**
- Create: `crates/agentd-store/src/execution_audit_repo.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/enterprise_execution_audit.rs`

- [x] **Step 1: Add audit ordering/idempotency/error tests and verify RED**

Cover reverse caller timestamps, exact retry, changed retry, mismatched links, cursor replay, unchanged sequence count, and SQL triggers.

- [x] **Step 2: Implement transactional append and replay, then verify GREEN**

Collapse exact `(scope,key)` retries onto the stored row, reject changed envelopes, and order replay only by autoincrement sequence.

### Task 5: Roadmap, Parity, and Contract Verification

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Create: `crates/agentctl/tests/enterprise_artifact_audit_contract.rs`

- [x] **Step 1: Add artifact test RED, update docs, verify GREEN**

Record migration 0014 as store-only evidence, keep parity partial, and advance Immediate Next Step to P269.

- [x] **Step 2: Run full regression/static verification**

Run `cargo fmt --all`, all targeted P268 tests, historical P264-P267 contract tests, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, and `git diff --check` for the declared change set.

- [x] **Step 3: Run P268 lifecycle, explain, and stamp**

Run lifecycle with every allowed path as an explicit `--change`, inspect `agent-spec explain`, and require stamp preview `9/9 passed`.
