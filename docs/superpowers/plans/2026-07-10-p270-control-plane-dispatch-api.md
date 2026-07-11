# P270 Control-Plane Dispatch API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a typed, durable control-plane task lease API with task-scoped monotonic fencing and worker-incarnation rejection.

**Architecture:** Keep lease identities, values, errors, requests, and the async port in `agentd-core`. Add an additive two-table SQLite model and `SqliteTaskLeaseControlPlane` in `agentd-store`; `BEGIN IMMEDIATE` transactions serialize grant allocation and keep immutable lease history synchronized with one durable task head.

**Tech Stack:** Rust, async-trait, serde, thiserror, SQLx SQLite, Tokio tests, agent-spec lifecycle.

## Global Constraints

- Preserve P200-P269 behavior and the shared dirty worktree.
- Never promote scheduler ticket/reservation values into canonical lease identity.
- Add no HTTP, MCP, Matrix, worker network, runtime, Specify, OpenFab, or external-service behavior.
- Tests use temporary SQLite and local fakes only; do not start Claude, Codex, tmux, Matrix, or remote services.
- Follow RED -> GREEN for each production slice and retain the observed failing command.

---

### Task 1: Core Lease Identity and Port

**Files:**
- Modify: `crates/agentd-core/src/types/ids.rs`
- Modify: `crates/agentd-core/src/types/enterprise.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-core/src/ports/task_lease.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-core/tests/task_lease.rs`

**Interfaces:**
- Produces: `LeaseId`, `FencingToken`, `LeaseStatus`, `TaskLeaseGrant`, `TaskLeaseClaim`, typed requests/rejections, and `TaskLeasePort`.

- [x] Write `task_lease_types_and_port_preserve_p265_contract` using a recording port and verify RED on missing imports.
- [x] Add the `ls_` id, validated non-zero token, lease status, request/result/error types, and six-method async port.
- [x] Run `cargo test -p agentd-core task_lease_types_and_port_preserve_p265_contract` and require GREEN.

### Task 2: Additive Lease Schema

**Files:**
- Create: `crates/agentd-store/migrations/0015_enterprise_task_leases.sql`
- Modify: `crates/agentd-store/tests/migration.rs`
- Modify: `crates/agentd-store/tests/migration_backcompat.rs`

**Interfaces:**
- Consumes: P265 id/state contract and P267 task/worker foreign keys.
- Produces: `execution_task_leases`, `execution_task_lease_heads`, schema version 15, constraints, indexes, and immutability/monotonicity triggers.

- [x] Add schema and backcompat tests and verify RED because migration 0015/tables do not exist.
- [x] Add the two-table migration, partial active index, immutable/terminal/no-delete triggers, head-current validation, and monotonic token trigger.
- [x] Update latest-schema assertions from 14 to 15 while preserving historical raw-migration assertions for 13 and 14.
- [x] Run both P270 migration selectors and existing migration suites; require GREEN.

### Task 3: Directed Dispatch and Monotonic Reacquisition

**Files:**
- Create: `crates/agentd-store/src/task_lease_control_plane.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Create: `crates/agentd-store/tests/enterprise_task_leases.rs`

**Interfaces:**
- Consumes: `TaskLeasePort`, task rows, worker/current-incarnation rows, and P270 schema.
- Produces: `SqliteTaskLeaseControlPlane::new(SqlitePool)`, transactional `dispatch`, grant reads, and conflict/expiry/supersession reconciliation.

- [x] Write first-dispatch and active-conflict/reacquisition tests; verify RED on the missing adapter.
- [x] Implement canonical id/time validation, `BEGIN IMMEDIATE`, unfinished-task and online-current-worker validation, active-row reconciliation, head increment, lease insertion, and head update.
- [x] Run the two dispatch selectors and require GREEN.

### Task 4: Claim Mutation, Expiry, and Reincarnation Fencing

**Files:**
- Modify: `crates/agentd-store/src/task_lease_control_plane.rs`
- Modify: `crates/agentd-store/tests/enterprise_task_leases.rs`

**Interfaces:**
- Consumes: exact `TaskLeaseClaim` and active grant state.
- Produces: renewal, release, cancellation, due expiry, claim validation, and stable typed rejection reasons.

- [x] Write exact-claim mutation, stale/terminal/expired, and reincarnation tests; verify RED on unimplemented methods.
- [x] Implement claim lookup against durable head/token/worker state, strict renewal extension, terminal transitions, due expiry, and superseded-incarnation reconciliation.
- [x] Run all three selectors and require GREEN.

### Task 5: Serialized Concurrent Dispatch

**Files:**
- Modify: `crates/agentd-store/tests/enterprise_task_leases.rs`
- Modify: `crates/agentd-store/src/task_lease_control_plane.rs`

**Interfaces:**
- Produces: one-winner behavior for simultaneous dispatch on the same task.

- [x] Add a barrier-based two-connection concurrent dispatch test and observe any duplicate grant or non-domain lock failure.
- [x] Keep `BEGIN IMMEDIATE` and busy-timeout behavior inside the adapter so the loser deterministically observes current active state and returns conflict.
- [x] Run the concurrency selector repeatedly and require one active row, one token, and one matching head each time.

### Task 6: Roadmap, Parity, and Contract Gates

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Create: `crates/agentctl/tests/control_plane_dispatch_api_contract.rs`
- Modify only if required by advanced evidence: historical P265/P267/P269/P263 artifact tests listed in the P270 boundary.

- [x] Add the P270 artifact-inspection test and verify RED before roadmap/parity changes.
- [x] Record P270 as durable API/storage evidence, keep replacement status partial, and advance Immediate Next Step to P271.
- [x] Run the P270 artifact selector and affected historical contract selectors; require GREEN.
- [x] Run `cargo fmt --all --check`, targeted tests, `cargo test --workspace`, and `cargo clippy --workspace --all-targets -- -D warnings`.
- [x] Run agent-spec parse/lint/contract, then lifecycle with explicit P270 change paths, explain, and dry-run stamp; require 10/10 acceptance passing without AI or external agents.
