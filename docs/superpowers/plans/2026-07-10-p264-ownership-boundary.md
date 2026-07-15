# P264 Ownership Boundary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Establish one source of truth for project, execution, worker-local, certification, and transport state before enterprise identities and schemas are added.

**Architecture:** Add a P264 ownership amendment with a machine-checked state table and protocol/deployment rules. Keep the historical Path B document, replacement roadmap, and parity map aligned by referencing the amendment instead of duplicating competing ownership tables.

**Tech Stack:** Markdown architecture contracts, Rust artifact integration tests, agent-spec lifecycle.

## Global Constraints

- This is a documentation-contract slice; add no production Rust or migrations.
- Preserve P200-P263 behavior and parity gate semantics.
- OpenFab certification defaults to `gate=none` and is separate from delivery.
- Enterprise project authority never silently falls back to local authority.

### Task 1: Contract and RED Tests

**Files:**
- Create: `specs/e2e/p264-agentd-specify-openfab-ownership-boundary.spec.md`
- Create: `docs/superpowers/plans/2026-07-10-p264-ownership-boundary.md`
- Create: `crates/agentctl/tests/enterprise_ownership_contract.rs`
- Modify: `crates/agentctl/tests/worktree_reconciliation_contract.rs`

- [x] **Step 1: Validate the P264 task contract**

Run parse, lint, and contract compilation; require a non-zero scenario count and
quality 1.0 with no diagnostics.

- [x] **Step 2: Run the seven ownership tests RED**

Run `cargo test -p agentctl --test enterprise_ownership_contract -- --nocapture`
before creating the ownership amendment. Expected: RED because the P264
amendment and its cross-document references do not exist. Run the P263
reconciliation test after adding the P264 spec and require RED on its stale
maximum-id assertion before correcting that historical assertion.

### Task 2: Authoritative Ownership Amendment

**Files:**
- Create: `docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md`

- [x] **Step 1: Define the five roles and 18 state classes**

Record one owner per state id, negative boundaries, the three protocol ports,
and standalone/enterprise authority precedence.

- [x] **Step 2: Run ownership-table selectors GREEN**

Run the first five P264 selectors. Cross-document tests may remain red until
Task 3.

### Task 3: Integrate Existing Architecture Artifacts

**Files:**
- Modify: `docs/specs/2026-05-29-agentd-specify-boundary.md`
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`
- Modify: `docs/parity/agent-chat-capability-map.md`

- [x] **Step 1: Add amendment references and owner labels**

Mark the Path B document as amended by P264, distinguish project authority from
execution control in Phase I, preserve the P263 immediate-step history while
advancing active work to P265, and annotate cross-system parity rows with
resolved owners.

- [x] **Step 2: Verify all P264 tests GREEN**

Run the seven P264 tests plus the P263 reconciliation contract and parity audit
regression tests.

### Task 4: Full Verification

- [x] **Step 1: Run regression and static checks**

Run agentctl tests, formatting, Clippy, and `git diff --check` for P264 paths.

- [x] **Step 2: Run lifecycle, explain, and stamp**

Run P264 lifecycle with all eight allowed paths explicit, inspect explain, and
require an 8/8 passing stamp preview with no skip or uncertain verdicts.
