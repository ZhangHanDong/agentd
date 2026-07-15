# P266 Project Authority Reference Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Define versioned project/repository/room authority references and one immutable execution snapshot without creating an agentd project authority database.

**Architecture:** Add an authoritative P266 document that refines P264 `ProjectAuthorityPort` and P265 `ProjectAuthorityRef`. Machine-check its catalogs and decision tables from `agentctl`, then update roadmap/parity while keeping runtime implementation status unchanged.

**Tech Stack:** Markdown architecture contracts, Rust artifact integration tests, agent-spec lifecycle.

## Global Constraints

- This is a documentation-contract slice; add no production Rust or migrations.
- Existing base project-related tables and columns remain unchanged compatibility data.
- Configured Specify failure never falls back to local authority.
- Repository paths and Matrix rooms are locators/transport, not project authority ids.
- Tests start no model, daemon, Matrix, Specify, OpenFab, or remote service.

### Task 1: Contract and RED Tests

**Files:**
- Create: `specs/e2e/p266-enterprise-project-room-repo-model.spec.md`
- Create: `docs/superpowers/plans/2026-07-10-p266-project-authority-reference.md`
- Create: `crates/agentctl/tests/enterprise_project_authority_contract.rs`

- [x] **Step 1: Validate the P266 task contract**

Run parse, lint, and contract compilation; require eight scenarios and quality
1.0 with no diagnostics.

- [x] **Step 2: Add eight artifact tests and verify RED**

Parse exact sets from the resource catalog, snapshot fields, recovery decision
matrix, and base compatibility table before the authority document exists.

### Task 2: Authoritative Project Reference Contract

**Files:**
- Create: `docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md`

- [x] **Step 1: Define the reference catalog and immutable snapshot**

Define authority/resource/version equality and all required snapshot fields,
including hash, expiry, policy versions, and offline recovery policy.

- [x] **Step 2: Define repository, room, recovery, and rebind rules**

Require one target repository/base commit, deterministic command binding,
fail-closed authority selection, bounded pinned recovery, and immutable rebind
history.

- [x] **Step 3: Classify base compatibility fields and run seven selectors GREEN**

Use the real base `agent_scheduler_queue.room` field rather than the sibling
worktree's obsolete `dispatch_queue.room` name.

### Task 3: Roadmap and Parity Integration

**Files:**
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`

- [x] **Step 1: Reference P266 and advance active work to P267**

Keep `project_room_repo_binding` missing because P266 adds no adapter or durable
authority implementation. Define P267 as the first enterprise agent/worker
schema slice and preserve earlier Immediate Next Step history.

- [x] **Step 2: Verify eight tests and historical contracts GREEN**

Run P266, P265, P264, P263, and parity tests.

### Task 4: Full Verification

- [x] **Step 1: Run regression and static checks**

Run agentctl tests, formatting, Clippy, and scoped `git diff --check`.

- [x] **Step 2: Run lifecycle, explain, and stamp**

Run P266 lifecycle with all six allowed paths explicit. Require eight passing
acceptance scenarios with no failed, skipped, or uncertain verdicts.
