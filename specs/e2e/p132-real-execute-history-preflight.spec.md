spec: task
name: "Real execute history preflight"
tags: [e2e, p0.9, real-agent, execute, smoke, github, safety]
---

## Intent

Fail the real `execute.dot` smoke before spending real agent work when the
current repository cannot produce a pull request against `origin/main`. P130
guards the final `open_pr` helper, but the full real execute harness should
surface the same no-common-history blocker during preflight, before daemon,
tmux, Claude, or agent processes start.

## Decisions

- Extend `scripts/agentd_real_execute_smoke.sh` preflight, not `execute.dot`.
- The harness fetches `origin/main` into `refs/remotes/origin/main` during
  preflight.
- The harness verifies that `origin/main` exists after fetch.
- The harness verifies `git merge-base origin/main HEAD` succeeds because
  `publish_branch` creates task branches from the current checked-out `HEAD`.
- The harness delegates the actual git history report to
  `scripts/agentd_pr_history_status.sh HEAD main`.
- If the history check fails, the harness exits non-zero before printing
  `preflight ok`, before starting the daemon, and stderr names both `HEAD` and
  `origin/main`.

## Boundaries

### Allowed Changes

- specs/e2e/p132-real-execute-history-preflight.spec.md
- specs/e2e/p131-real-execute-smoke-harness.spec.md
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/real_execute_smoke.rs
- docs/p0.9-deployment-checklist.md

### Forbidden

- Do not rewrite local or remote git history.
- Do not change `scripts/agentd_open_pr.sh` semantics in this slice.
- Do not start real daemon, tmux, Claude, agent, or GitHub PR creation from
  automated tests.
- Do not attempt to repair the current remote-history mismatch automatically.
- Do not modify or delete existing `.agentd/real-execute-smoke/*` evidence.

## Out of Scope

- Choosing a base branch other than `origin/main`.
- Adding automatic branch rebasing or migration.
- Changing `publish_branch` or `open_pr` workflow topology.
- Solving Claude account quota or GitHub account authentication.

## Completion Criteria

Scenario: preflight rejects no common history before agents
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_preflight_rejects_no_common_history_before_agents
  Level: script integration
  Test Double: fake PATH with git merge-base failure
  Given fake local prerequisites pass and fake `git fetch origin main` succeeds
  And fake `git rev-parse origin/main` succeeds
  And fake `git merge-base origin/main HEAD` exits non-zero
  When the harness runs with `--preflight-only`
  Then it exits non-zero
  And stderr names `HEAD` and `origin/main`
  And stdout does not contain `preflight ok`
  And no daemon log exists in the state directory

Scenario: preflight still accepts compatible history readiness
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_preflight_accepts_fake_tools
  Level: script integration
  Test Double: fake PATH with local tool shims
  Given fake local prerequisites pass
  And fake git fetch, rev-parse, and merge-base checks succeed
  When the harness runs with `--preflight-only`
  Then it exits 0
  And stdout contains `preflight ok`
  And no daemon log exists in the state directory

Scenario: dry-run documents the history preflight guard
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_mentions_history_preflight
  Level: script integration
  Test Double: process invocation in dry-run mode
  Given no opt-in environment variable is set
  When the harness runs with `--dry-run`
  Then stdout names the `origin/main` and `HEAD` common-history preflight
