spec: task
name: "Real execute smoke status docs"
tags: [docs, p0-9, p2, real-execute, status]
---

## Intent

Record the actual 2026-07-07 real execute smoke outcome in the P0.9 checklist
and P2 plan. The run verified the execute chain through implement, review,
aggregate, and branch publication, but it did not complete the full real-agent
end-to-end smoke because the real agent hit a spend-limit blocker and the run
failed at `open_pr` on a no-common-history GitHub preflight.

## Decisions

- `docs/p0.9-deployment-checklist.md` must record the latest real execute smoke
  as `partial_execute_chain_verified_publish_ok_pr_blocked`, not as untouched
  and not as fully complete.
- `docs/plans/2026-06-06-agentd-p2-plan.md` must distinguish local worktree
  activation from the remaining real execute environment gate, while naming the
  partial real-env evidence.
- The docs must name the real blocker: `open_pr` failed because the published
  branch had no common history with `origin/main`.
- The docs must name the real-agent limitation: Claude started but a monthly
  spend limit blocked implementation/review, so the operator manually submitted
  implement and review outcomes through agentd MCP stdio.
- Verification is source-inspection only. Do not run or require another
  `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` attempt.

## Boundaries

### Allowed Changes

- **/docs/p0.9-deployment-checklist.md
- **/docs/plans/2026-06-06-agentd-p2-plan.md
- **/specs/e2e/p148-real-execute-status-docs.spec.md
- **/crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not commit additional `.agentd/real-execute-smoke/**` runtime artifacts.
- Do not change runtime Rust code.
- Do not claim the full real execute smoke or real-agent path completed.
- Do not run `AGENTD_REAL_EXECUTE_SMOKE=1 --execute`.

### Out of Scope

- Fixing the GitHub no-common-history/open_pr blocker.
- Re-running real Claude, real execute, or real SIGKILL smokes.
- Specify HTTP/WS transport or external API contract work.

## Completion Criteria

Scenario: P0.9 checklist records the partial real execute evidence
  Test: deployment_checklist_records_partial_real_execute_attempt
  Given the P0.9 deployment checklist
  When the source-inspection test reads it
  Then it names `partial_execute_chain_verified_publish_ok_pr_blocked`,
  the `real-execute-smoke-20260707070439` run id, the `failed_at_open_pr`
  terminal point, the no-common-history blocker, and the manual MCP outcome
  submission caused by the Claude spend-limit blocker

Scenario: P2 plan keeps the full real execute gate open
  Test: p2_plan_records_real_execute_partial_not_complete
  Given the P2 plan
  When the source-inspection test reads it
  Then it records the partial real execute evidence without claiming the full
  `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` path completed
