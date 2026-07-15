# P263 Worktree Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Freeze the P200-P262 base worktree as the integration authority and map every conflicting sibling P202-P228 capability into covered, port-required, integrated, or renumbered work.

**Architecture:** Add a durable reconciliation manifest, amend the existing parity map and roadmap, and bind the contracts with artifact tests. This slice intentionally changes no production code or database schema.

**Tech Stack:** Markdown contracts, Rust artifact tests, agent-spec lifecycle.

## Global Constraints

- Do not modify or clean the sibling replacement worktree.
- Preserve all current base implementation and spec evidence.
- Do not copy migration files with conflicting version prefixes.
- Do not start real agents, Matrix, tmux, or external services.

### Task 1: Contract and RED Tests

**Files:**
- Create: `specs/e2e/p263-agent-chat-worktree-reconciliation.spec.md`
- Create: `docs/superpowers/plans/2026-07-10-p263-worktree-reconciliation.md`
- Create: `crates/agentctl/tests/worktree_reconciliation_contract.rs`

- [x] **Step 1: Validate the P263 task contract**

Run parse, lint, and contract compilation; require quality 1.0 and no diagnostics.

- [x] **Step 2: Run reconciliation tests RED**

Run `cargo test -p agentctl --test worktree_reconciliation_contract` before creating the manifest/amendments. Expected: RED because P263 artifacts and enterprise rows do not exist.

### Task 2: Reconciliation Manifest

**Files:**
- Create: `docs/parity/agent-chat-worktree-reconciliation.md`

- [x] **Step 1: Map all source specs and migrations**

Record exactly one disposition for P202-P228, the exact five port-required behaviors, P223-P228 renumbering, and base-compatible migration reservations.

- [x] **Step 2: Verify mapping tests GREEN**

Run the two P263 reconciliation/sequence selectors.

### Task 3: Parity and Roadmap Amendment

**Files:**
- Modify: `docs/parity/agent-chat-capability-map.md`
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`

- [x] **Step 1: Add enterprise rows and reconciled phase sequence**

Preserve historical evidence, add nine blocking enterprise/native rows, reserve P264-P279, and make P264 the immediate next step.

- [x] **Step 2: Verify all P263 tests GREEN**

Run the full P263 artifact test and the existing parity audit gate test.

### Task 4: Full Verification

- [x] **Step 1: Run regression and static checks**

Run agentctl tests, workspace format/clippy, and `git diff --check` for P263 paths.

- [x] **Step 2: Run lifecycle, explain, and stamp**

Run P263 lifecycle with all six allowed paths explicit, inspect explain, and require stamp preview 5/5.
