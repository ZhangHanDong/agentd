# P271 Control-Plane Artifact Audit API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Expose P268 execution evidence through typed, bounded control-plane ports and fence worker artifact/usage reports with P270 lease claims and durable rejection audit.

**Architecture:** Define shared evidence envelopes and four async ports in `agentd-core`. Extend the P268 repositories with stable bounded queries, then implement `SqliteExecutionEvidenceControlPlane<L>` over those repositories and an injected `TaskLeasePort`; usage is a typed `usage.measured` audit event, so P271 requires no schema migration.

**Tech Stack:** Rust, async-trait, serde/serde_json, SHA-256, SQLx SQLite, Tokio tests, agent-spec lifecycle.

## Global Constraints

- Keep migration 0015 latest; P271 adds no table, column, trigger, or row rewrite.
- Keep OpenFab certification authority external and default delivery gate `none`.
- Require current exact P270 claims for worker artifact and usage reports, and persist rejection audit before returning typed rejection.
- Add no object bytes, network transport, daemon route, CLI command, runtime, policy enforcement, or external service startup.
- Preserve the shared dirty worktree and follow RED -> GREEN for every behavior slice.

---

### Task 1: Core Evidence Types and Ports

**Files:**
- Create: `crates/agentd-core/src/ports/execution_evidence.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-core/tests/execution_evidence.rs`

**Interfaces:**
- Produces: closed kinds, `ExecutionEvidenceLinks`, artifact/audit/usage/certification requests and records, stable pages/cursors, `ExecutionEvidenceError`, and four async ports.

- [x] Write the recording-port contract test and verify RED on missing imports.
- [x] Implement the minimal serializable API values, validation helpers, and port traits.
- [x] Run the core selector and require GREEN.

### Task 2: Bounded P268 Repository Queries

**Files:**
- Modify: `crates/agentd-store/src/execution_artifact_repo.rs`
- Modify: `crates/agentd-store/src/execution_audit_repo.rs`

**Interfaces:**
- Produces: artifact exact-envelope comparison, stable run-page query, certification ref listing, and bounded audit sequence replay without changing P268 writes.

- [x] Add adapter tests for artifact, audit, and certification APIs and verify RED on the missing control-plane adapter/query functions.
- [x] Add focused repository helpers with `1..=200` limits and stable order.
- [x] Implement exact artifact retry handling and port conversions in the adapter.
- [x] Run artifact/audit/certification selectors plus P268 tests and require GREEN.

### Task 3: Typed Usage Ledger on Audit Events

**Files:**
- Modify: `crates/agentd-store/src/execution_audit_repo.rs`
- Create: `crates/agentd-store/src/execution_evidence_control_plane.rs`
- Modify: `crates/agentd-store/src/lib.rs`
- Modify: `crates/agentd-store/Cargo.toml`
- Create: `crates/agentd-store/tests/control_plane_execution_evidence.rs`

**Interfaces:**
- Consumes: audit append/replay and closed `UsageMetric` payload.
- Produces: typed usage record, page, and deterministic totals with exact audit idempotency.

- [x] Write usage append/retry/page/totals tests and verify RED.
- [x] Implement usage-to-audit conversion, strict payload parsing, sequence paging, and checked totals; add SHA-256 only for generated rejection audit payloads.
- [x] Prove no usage table exists and rerun usage plus P268 audit selectors GREEN.

### Task 4: Fenced Worker Evidence and Rejection Audit

**Files:**
- Modify: `crates/agentd-store/src/execution_evidence_control_plane.rs`
- Modify: `crates/agentd-store/tests/control_plane_execution_evidence.rs`

**Interfaces:**
- Consumes: `TaskLeasePort::validate_claim`, worker report links, and audit append.
- Produces: accepted worker artifact/usage records or typed lease rejection after durable `execution.report_rejected` append.

- [x] Write active, stale-token, superseded-incarnation, and terminal-lease report tests and verify RED.
- [x] Implement link matching, lease validation, rejection payload hashing, fail-closed audit append, and no-evidence-on-rejection behavior.
- [x] Run both fenced selectors and P270 lease tests; require GREEN.

### Task 5: Roadmap, Parity, and Final Gates

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Create: `crates/agentctl/tests/control_plane_execution_evidence_api_contract.rs`
- Modify affected historical artifact gates only when evidence text advances.

- [x] Add the P271 artifact gate and verify RED before roadmap/parity edits.
- [x] Record the four APIs, usage-on-audit design, and fenced rejection audit while keeping upload/OpenFab/cutover partial; advance Immediate Next Step to P272.
- [x] Run affected historical artifact/ownership/P270/P263 gates and require GREEN.
- [x] Run format, targeted tests, `cargo test --workspace`, and workspace Clippy.
- [x] Run agent-spec lifecycle with explicit P271 paths, then explain and dry-run stamp; require 8/8 acceptance passing without AI or external agents.
