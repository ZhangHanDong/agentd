spec: task
name: "P2 worktree activation status docs"
tags: [docs, p2, worktree, status]
---

## Intent

Align the P2 implementation plan and the p11 worktree task spec with the
current as-built worktree activation. The code now injects a real
`WorktreeAllocator` in the daemon and `execute.dot` consumes `${worktree}`, so
the docs must stop describing p11/R3a as a draft-only inert mechanism or P2 as
uncommitted planning.

## Decisions

- `docs/plans/2026-06-06-agentd-p2-plan.md` records P2 status as as-built plus
  remaining gates, not uncommitted planning.
- The P2 plan names the completed worktree activation path: daemon
  `WorktreePool` injection, `${worktree}` in `execute.dot`, worktree publication,
  reviewer snapshots, release/cleanup, and the remaining real execute smoke
  gate.
- `specs/core/p11-per-task-run-worktree.spec.md` remains the R3a mechanism
  contract, but no longer claims the current daemon passes `None`, that
  `execute.dot` is unmigrated, or that implementation is blocked by a pending
  advisor review.
- Verification is source-inspection only; no runtime behavior changes are made.

## Boundaries

### Allowed Changes

- **/docs/plans/2026-06-06-agentd-p2-plan.md
- **/specs/core/p11-per-task-run-worktree.spec.md
- **/specs/e2e/p147-p2-worktree-status-docs.spec.md
- **/crates/agentd-core/tests/p2_docs.rs

### Forbidden

- Do not change runtime Rust code outside source-inspection tests.
- Do not run or require any `AGENTD_REAL_* --execute` smoke.
- Do not invent a Specify HTTP/WS contract.

## Completion Criteria

Scenario: P2 plan records the worktree activation as current
  Test: p2_plan_records_worktree_activation_as_built
  Given the P2 plan and the daemon/workflow source files
  When the source-inspection test reads them
  Then the plan states as-built status, names daemon `WorktreePool` injection,
  and names `${worktree}` use in `execute.dot`

Scenario: P2 plan keeps real execute smoke as the remaining gate
  Test: p2_plan_keeps_real_execute_smoke_as_remaining_gate
  Given the P2 plan
  When the source-inspection test reads it
  Then it names the real execute smoke gate without claiming `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` has been run

Scenario: p11 no longer presents implemented activation as draft-only
  Test: p11_spec_no_longer_claims_r3a_is_unimplemented
  Given the p11 task spec and current source files
  When the source-inspection test reads them
  Then the spec does not contain the stale DRAFT/advisor-review blocker,
  does not claim the daemon keeps passing `None`, and does not claim
  `execute.dot` remains unmigrated
