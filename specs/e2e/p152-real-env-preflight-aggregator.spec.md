spec: task
name: "Real environment preflight aggregator"
tags: [e2e, scripts, real-execute, real-sigkill, safety]
---

## Intent

Add one safe operator entry point for checking real-environment readiness before
the remaining P0.9 gates. The helper must aggregate the existing git-history,
real Claude, real execute, and real SIGKILL preflights while preserving the
project rule that real `--execute` gates require explicit per-harness opt-in.

## Decisions

- Add `scripts/agentd_real_env_preflight.sh` as a non-executing aggregate
  helper.
- The helper supports `--dry-run` and `--preflight-only`; default mode is
  `--dry-run`.
- The helper rejects `--execute` instead of forwarding it to any underlying
  harness.
- `--preflight-only` runs `agentd_pr_history_status.sh HEAD main` first, then
  the real Claude, real execute, and real SIGKILL harnesses in
  `--preflight-only` mode.
- Failure in an earlier preflight stops the sequence before later real-agent or
  daemon-related checks are invoked.
- The P0.9 deployment checklist should point operators to the aggregate helper
  as a safe readiness check, without implying any real gate completed.

## Boundaries

### Allowed Changes

- **/specs/e2e/p152-real-env-preflight-aggregator.spec.md
- **/scripts/agentd_real_env_preflight.sh
- **/crates/agentd-bin/tests/real_env_preflight.rs
- **/docs/p0.9-deployment-checklist.md

### Forbidden

- Do not run or add any automatic `AGENTD_REAL_* --execute` path.
- Do not change the behavior of the existing real Claude, real execute, or real
  SIGKILL harnesses.
- Do not start daemon, tmux, Claude, GitHub PR creation, or SIGKILL behavior from
  the aggregate helper.
- Do not commit `.agentd/**` runtime artifacts.

## Out of Scope

- Running the real execute smoke.
- Running the real SIGKILL smoke.
- Running the 90-second demo.
- Solving external Claude quota, GitHub authentication, or PR creation blockers.

## Completion Criteria

Rule: real-env-preflight-aggregator

Scenario: dry-run prints the aggregate plan without creating state
  Test:
    Package: agentd-bin
    Filter: real_env_preflight_dry_run_prints_plan_without_starting
  Level: script integration
  Test Double: temporary state directory
  Given `agentd_real_env_preflight.sh --dry-run`
  When the test reads stdout
  Then the plan names `agentd_pr_history_status.sh HEAD main`
  And it names the three component `--preflight-only` harnesses
  And no state directory or daemon artifact is created

Scenario: preflight-only succeeds with fake local prerequisites
  Test:
    Package: agentd-bin
    Filter: real_env_preflight_preflight_only_accepts_fake_prereqs
  Level: script integration
  Test Double: fake local tools on PATH
  Given fake `cargo`, `tmux`, `claude`, `agent-spec`, `curl`, `git`, `gh`, and
  `sqlite3` tools that satisfy the component preflight checks
  When `agentd_real_env_preflight.sh --preflight-only` runs
  Then it reports each component preflight as complete
  And it reports that no `AGENTD_REAL_* --execute` command ran
  And no daemon artifact is created

Scenario: execute mode is refused at the aggregate boundary
  Test:
    Package: agentd-bin
    Filter: real_env_preflight_rejects_execute_mode
  Level: script integration
  Test Double: none
  Given the aggregate helper
  When it is invoked with `--execute`
  Then it exits non-zero
  And stderr says the helper never runs `AGENTD_REAL_* --execute` gates

Scenario: git-history failure stops before agent preflights
  Test:
    Package: agentd-bin
    Filter: real_env_preflight_history_failure_stops_before_agent_checks
  Level: script integration
  Test Double: fake local tools with `git merge-base` failure
  Given `agentd_pr_history_status.sh HEAD main` reports no common history
  When `agentd_real_env_preflight.sh --preflight-only` runs
  Then it exits non-zero
  And stdout does not report the real Claude, real execute, or real SIGKILL
  component preflights as complete

Scenario: deployment checklist points to the safe aggregate helper
  Test:
    Package: agentd-bin
    Filter: real_env_preflight_deployment_checklist_mentions_aggregate_helper
  Level: source inspection
  Test Double: documentation text only
  Given the P0.9 deployment checklist
  When the source-inspection test reads it
  Then it mentions `agentd_real_env_preflight.sh --preflight-only`
  And it says the aggregate helper does not run `AGENTD_REAL_* --execute` gates
