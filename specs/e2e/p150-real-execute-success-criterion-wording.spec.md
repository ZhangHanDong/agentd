spec: task
name: "Real execute success criterion wording"
tags: [e2e, docs, real-execute, safety]
---

## Intent

Keep the real execute smoke harness plan aligned with the P148/P149 status
model. A captured `open_pr` preflight error is useful evidence, but the full real
execute smoke is successful only when the run finishes after real agents,
branch publication, and real PR creation.

## Decisions

- Update only the dry-run plan wording in `scripts/agentd_real_execute_smoke.sh`;
  do not change execute-mode behavior.
- The success criterion must say that `open_pr` opens a real PR.
- A captured `scripts/agentd_open_pr.sh` preflight error must be described as
  evidence from a failed run, not as an alternative success condition.
- Do not run any `AGENTD_REAL_EXECUTE_SMOKE=1 --execute` path for this wording
  change.

## Boundaries

### Allowed Changes

- **/specs/e2e/p150-real-execute-success-criterion-wording.spec.md
- **/scripts/agentd_real_execute_smoke.sh
- **/crates/agentd-bin/tests/real_execute_smoke.rs

### Forbidden

- Do not change real execute runtime behavior.
- Do not change the harness opt-in gates.
- Do not change `scripts/agentd_open_pr.sh` or PR history helpers.
- Do not run `AGENTD_REAL_EXECUTE_SMOKE=1 --execute`.

## Out of Scope

- Re-running the real execute smoke.
- Solving Claude account quota or GitHub authentication.
- Changing P0.9 checklist status or P2 plan status.

## Completion Criteria

Rule: real-execute-success-wording

Scenario: dry-run plan distinguishes PR success from captured preflight failure
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_distinguishes_pr_success_from_captured_preflight_failure
  Level: script integration
  Test Double: dry-run mode only
  Given `agentd_real_execute_smoke.sh --dry-run`
  When the test reads the printed plan
  Then the success criterion says `open_pr opens a real PR`
  And the plan says a captured preflight error from `scripts/agentd_open_pr.sh`
  is failure evidence rather than success
  And the plan does not say `open_pr either opens a PR or fails`
