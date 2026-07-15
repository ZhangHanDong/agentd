# P269 Control-Plane Project Authority API Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add the P266 project-authority domain/port plus explicit local and fail-closed Specify adapters and control-plane pin/recovery decisions.

**Architecture:** Keep immutable reference/snapshot types and the async port in `agentd-core`. Add `agentd-project-authority` for adapters and orchestration; the Specify adapter wraps an injected transport and deliberately defines no network wire contract.

**Tech Stack:** Rust, async-trait, serde, thiserror, Tokio tests, agent-spec lifecycle.

## Global Constraints

- Add no project authority table or migration.
- Add no Specify HTTP client, credentials, network request, or service startup.
- Configured Specify failure is fail-closed and never selects local authority.
- Tests use in-memory snapshots and fake traits only; no external agent/runtime/service starts.
- Preserve P200-P268 behavior and the shared dirty worktree.

---

### Task 1: Core Authority Domain and Port

**Files:**
- Create: `crates/agentd-core/src/types/project_authority.rs`
- Modify: `crates/agentd-core/src/types/mod.rs`
- Create: `crates/agentd-core/src/ports/project_authority.rs`
- Modify: `crates/agentd-core/src/ports/mod.rs`
- Create: `crates/agentd-core/tests/project_authority.rs`

**Interfaces:**
- Produces: typed P266 refs, `ProjectExecutionSnapshot::validate`, `ProjectAuthorityPort`, resolve/health/error values.

- [x] Write the complete snapshot/reference test and verify RED on missing types.
- [x] Implement closed kinds, typed wrappers, bindings, validation, and port types.
- [x] Rerun the core selector and require GREEN.

### Task 2: Explicit Local Adapter

**Files:**
- Modify: `Cargo.toml`
- Create: `crates/agentd-project-authority/Cargo.toml`
- Create: `crates/agentd-project-authority/src/lib.rs`
- Create: `crates/agentd-project-authority/src/local.rs`
- Create: `crates/agentd-project-authority/tests/support/mod.rs`
- Create: `crates/agentd-project-authority/tests/local_project_authority.rs`

**Interfaces:**
- Consumes: core snapshot validation and port.
- Produces: `LocalProjectAuthority::new`, explicit current-project resolve, exact refresh, local health.

- [x] Write local happy/configuration tests and verify RED on missing crate/adapter.
- [x] Add the workspace crate and minimal immutable local adapter.
- [x] Rerun both local selectors and require GREEN.

### Task 3: Fail-Closed Specify Adapter Boundary

**Files:**
- Create: `crates/agentd-project-authority/src/specify.rs`
- Modify: `crates/agentd-project-authority/src/lib.rs`
- Create: `crates/agentd-project-authority/tests/specify_project_authority.rs`

**Interfaces:**
- Produces: `SpecifyAuthorityTransport`, `SpecifyProjectAuthority<T>`, forwarding and response validation.

- [x] Write forwarding, invalid-envelope, and unavailable tests and verify RED.
- [x] Implement transport delegation without network code or local fallback state.
- [x] Rerun both Specify selectors and require GREEN.

### Task 4: Control-Plane Pin and Recovery Decisions

**Files:**
- Create: `crates/agentd-project-authority/src/control_plane.rs`
- Modify: `crates/agentd-project-authority/src/lib.rs`
- Create: `crates/agentd-project-authority/tests/control_plane_authority.rs`

**Interfaces:**
- Produces: `ProjectAuthorityControlPlane<P>`, `PinnedProjectSnapshot`, `RecoveryInputs`, and `RecoveryAuthorization`.

- [x] Write new-execution and recovery decision tests and verify RED.
- [x] Implement resolve-time pinning and exact live/bounded-offline recovery.
- [x] Rerun both control-plane selectors and require GREEN.

### Task 5: Roadmap, Parity, and Contract Gates

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `crates/agentctl/tests/enterprise_project_authority_contract.rs`
- Create: `crates/agentctl/tests/project_authority_api_contract.rs`
- Modify: `crates/agentctl/tests/worktree_reconciliation_contract.rs`

- [x] Add the P269 artifact test, observe RED, update docs, and require GREEN plus P266-P268 historical tests.
- [x] Run format, Clippy, targeted tests, `cargo test --workspace`, and scoped diff checks.
- [x] Run lifecycle with every allowed path, then explain and stamp with 8/8 acceptance passing.
