# P265 Runtime Worker Identity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Freeze distinct enterprise identities, relationships, lifecycle states, and fencing/recovery rules before schema implementation.

**Architecture:** Add one authoritative P265 identity document and machine-check it from an `agentctl` artifact integration test. Keep current runtime/storage behavior unchanged; the replacement roadmap and parity documentation point later slices at the contract.

**Tech Stack:** Markdown architecture contracts, Rust artifact integration tests, agent-spec lifecycle.

## Global Constraints

- This is a documentation-contract slice; add no production Rust or migrations.
- Preserve P200-P264 runtime behavior and parity gate semantics.
- Legacy agent, tmux, process, host, Matrix, and dispatch values are never new canonical ids.
- Tests use repository files only and start no agent or external service.

### Task 1: Contract and RED Tests

**Files:**
- Create: `specs/e2e/p265-enterprise-runtime-worker-identity-contract.spec.md`
- Create: `docs/superpowers/plans/2026-07-10-p265-runtime-worker-identity.md`
- Create: `crates/agentctl/tests/enterprise_identity_contract.rs`

- [x] **Step 1: Validate the P265 task contract**

Run parse, lint, and contract compilation; require eight scenarios and quality
1.0 with no diagnostics.

- [x] **Step 2: Add eight artifact tests and verify RED**

Parse exact identity, legacy classification, relationship, and lifecycle tables.
Run `cargo test -p agentctl --test enterprise_identity_contract -- --nocapture`
before the identity document exists and require a missing-document failure.

### Task 2: Authoritative Identity Contract

**Files:**
- Create: `docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md`

- [x] **Step 1: Define identities and legacy classifications**

Define `AgentProfileId`, `WorkerId`, `WorkerIncarnationId`,
`RuntimeSessionId`, `RuntimeAttemptId`, `ExecutionRunId`, `ExecutionTaskId`, and
`LeaseId`, then classify current runtime/dispatch fields as aliases, locators,
metadata, or import inputs.

- [x] **Step 2: Define relationships, lifecycle states, and failure rules**

Document worker reincarnation, runtime resume/loss, lease fencing, terminal
state immutability, foreign authority references, and explicit new identities
on retry, resume, or re-registration.

- [x] **Step 3: Run identity-document selectors GREEN**

The first seven selectors must pass while roadmap/parity integration remains
red until Task 3.

### Task 3: Roadmap and Parity Integration

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`

- [x] **Step 1: Reference P265 and advance active work to P266**

Keep `durable_runtime_identity` partial because documentation is not runtime
implementation. Preserve P263/P264 immediate-step history while naming P266 as
the next foreign project-authority reference slice.

- [x] **Step 2: Verify eight tests and historical contracts GREEN**

Run P265, P264 ownership, P263 reconciliation, and parity CLI tests.

### Task 4: Full Verification

- [x] **Step 1: Run regression and static checks**

Run agentctl tests, formatting, Clippy, and scoped `git diff --check`.

- [x] **Step 2: Run lifecycle, explain, and stamp**

Run P265 lifecycle with all six allowed paths explicit. Require eight passing
acceptance scenarios with no failed, skipped, or uncertain verdicts.
