spec: task
name: "PR history status helper"
tags: [e2e, p0.9, git, pr, safety, operator]
---

## Intent

Make the current PR history blocker diagnosable without running the full real
execute preflight. P130 and P132 reject no-common-history states, but operators
need a git-only command that reports the compared refs and merge-base status
before they spend time on Claude, tmux, GitHub auth, or a full agent run.

## Decisions

- Add `scripts/agentd_pr_history_status.sh [head_ref] [base_branch]`.
- `head_ref` defaults to `HEAD`; `base_branch` defaults to `main`; the remote
  remains `origin`.
- The helper fetches `origin/<base_branch>` into the matching remote-tracking
  ref, verifies both refs resolve to commits, and checks `git merge-base
  origin/<base_branch> <head_ref>`.
- The helper prints a stable stdout report containing `head_ref`, `head_sha`,
  `base_ref`, `base_sha`, and `merge_base`.
- When refs have no common history, the helper exits non-zero, prints
  `merge_base: none`, and stderr names both compared refs.
- The helper reports only through stdout and stderr; it writes no report file.
- The helper never pushes, rewrites history, invokes `gh`, starts daemon/tmux,
  or runs an agent.
- `scripts/agentd_real_execute_smoke.sh` reuses the helper for its `HEAD` versus
  `origin/main` history preflight.

## Boundaries

### Allowed Changes

- specs/e2e/p133-pr-history-status-helper.spec.md
- specs/e2e/p132-real-execute-history-preflight.spec.md
- scripts/agentd_pr_history_status.sh
- scripts/agentd_real_execute_smoke.sh
- crates/agentd-bin/tests/pr_history_status.rs
- crates/agentd-bin/tests/real_execute_smoke.rs
- docs/p0.9-deployment-checklist.md

### Forbidden

- Do not rewrite local or remote git history.
- Do not change `scripts/agentd_open_pr.sh` behavior.
- Do not add a new base-branch policy for workflows.
- Do not invoke `gh`, Claude, tmux, daemon, or agents from the helper.
- Do not modify or delete existing `.agentd/real-execute-smoke/*` evidence.

## Out of Scope

- Automatically repairing unrelated histories.
- Choosing a base branch other than `origin/main` for shipped workflows.
- Retrying GitHub PR creation.
- Solving Claude account quota or GitHub authentication.

## Completion Criteria

<!-- lint-ack: error-path — the no-common-history scenario is the explicit failure path. -->
<!-- lint-ack: output-mode-coverage — this helper intentionally has stdout/stderr only and no file-output mode. -->
<!-- lint-ack: boundary-entry-point — real_execute_smoke.rs is covered by the source-inspection scenario that binds real_execute_smoke_preflight_uses_pr_history_status_helper. -->

Scenario: helper reports no common history without gh
  Test:
    Package: agentd-bin
    Filter: pr_history_status_reports_no_common_history_without_gh
  Level: script integration
  Test Double: temporary git repositories
  Given `origin/main` and local `HEAD` point to unrelated histories
  And `PATH` contains no fake `gh` requirement
  When `agentd_pr_history_status.sh` runs
  Then it exits non-zero
  And stdout contains `head_ref: HEAD`, `base_ref: origin/main`, and `merge_base: none`
  And stderr names the missing common history between `HEAD` and `origin/main`

Scenario: helper reports merge base for compatible history
  Test:
    Package: agentd-bin
    Filter: pr_history_status_reports_merge_base_for_compatible_history
  Level: script integration
  Test Double: temporary git repositories
  Given local `HEAD` descends from `origin/main`
  When `agentd_pr_history_status.sh` runs
  Then it exits 0
  And stdout contains `head_ref: HEAD`, `base_ref: origin/main`, `head_sha`, `base_sha`, and `merge_base`
  And stdout does not contain `merge_base: none`

Scenario: real execute preflight reuses the helper
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_preflight_uses_pr_history_status_helper
  Level: source inspection
  Test Double: crates/agentd-bin/tests/real_execute_smoke.rs source inspection
  Given the real execute smoke harness source is inspected
  When its git history preflight implementation is checked
  Then it invokes `scripts/agentd_pr_history_status.sh` with `HEAD` and `main`
