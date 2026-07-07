spec: task
name: "Real environment preflight status docs"
tags: [docs, p0-9, p2, real-execute, real-sigkill, status, safety]
---

## Intent

Record the safe, non-executing post-P150 readiness checks without converting
them into real-environment completion claims. The current `main` branch has
passed dry-run/preflight-only checks for the real Claude, real execute, and real
SIGKILL harnesses, and PR history status reports a valid merge-base, but no
`AGENTD_REAL_* --execute` gate has been run.

## Decisions

- `docs/p0.9-deployment-checklist.md` must record the post-P150 safe preflight
  evidence as readiness evidence only.
- `docs/plans/2026-06-06-agentd-p2-plan.md` must keep the real execute, real
  SIGKILL, and demo gates open while naming the safe preflight evidence.
- The docs must name `main` at `0e42750`, the `agentd_pr_history_status.sh HEAD
  main` merge-base result, and the three harness preflight-only checks.
- The docs must explicitly say no `AGENTD_REAL_* --execute` path was run.

## Boundaries

### Allowed Changes

- **/specs/e2e/p151-real-env-preflight-status-docs.spec.md
- **/docs/p0.9-deployment-checklist.md
- **/docs/plans/2026-06-06-agentd-p2-plan.md
- **/crates/agentd-bin/tests/deployment_checklist.rs
- **/crates/agentd-core/tests/p2_docs.rs

### Forbidden

- Do not run any `AGENTD_REAL_* --execute` command.
- Do not change real harness behavior.
- Do not claim the full real execute smoke, real SIGKILL smoke, or demo gate
  completed.
- Do not commit runtime artifacts under `.agentd/**`.

## Out of Scope

- Retrying the real execute smoke.
- Running the real SIGKILL drill.
- Running the 90-second demo.
- Solving external agent quota, GitHub authentication, or PR creation blockers.

## Completion Criteria

<!-- lint-ack: error-path - this is a documentation status-only task; the scenarios assert non-completion wording and forbid treating preflight evidence as execute success. -->

Rule: real-env-preflight-status-docs

Scenario: P0.9 checklist records post-P150 preflight readiness only
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_records_post_p150_safe_preflights
  Level: source inspection
  Test Double: documentation text only
  Given the P0.9 deployment checklist
  When the source-inspection test reads it
  Then it records the post-P150 safe preflight evidence on `main` at `0e42750`
  And it names the real Claude, real execute, and real SIGKILL preflight-only
  checks plus the `agentd_pr_history_status.sh HEAD main` merge-base status
  And it says no `AGENTD_REAL_* --execute` gate ran
  And it keeps the real-environment gates open

Scenario: P2 plan records preflight readiness without closing gates
  Test:
    Package: agentd-core
    Filter: p2_plan_records_post_p150_preflight_readiness_not_completion
  Level: source inspection
  Test Double: documentation text only
  Given the P2 plan
  When the source-inspection test reads it
  Then it records the same post-P150 safe preflight evidence
  And it does not claim the real execute, real SIGKILL, or demo gate completed
