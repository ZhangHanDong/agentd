spec: task
name: "PR history bridge helper"
tags: [e2e, p0.9, git, pr, safety, operator]
---

## Intent

Give operators a guarded local repair path for the current no-common-history PR
blocker. P133 reports that `HEAD` and `origin/main` have no merge-base; this
slice adds an explicit opt-in helper that can create that merge-base locally
without rewriting remote history, pushing branches, invoking GitHub, or starting
agents.

## Decisions

- Add `scripts/agentd_pr_history_bridge.sh [--dry-run|--execute] [base_branch]`.
- The helper only operates on the current checked-out `HEAD`; the base branch
  defaults to `main` on remote `origin`.
- `--dry-run` is the default mode and prints the exact local merge command it
  would run when no merge-base exists.
- Real bridge execution requires both `--execute` and
  `AGENTD_PR_HISTORY_BRIDGE=1`.
- Execute mode fetches `origin/<base_branch>`, refuses a dirty worktree, and
  then runs `git merge --allow-unrelated-histories --no-edit origin/<base_branch>`
  only when `git merge-base origin/<base_branch> HEAD` currently fails.
- If history is already compatible, the helper exits 0 without creating a new
  commit.
- The helper never pushes, rewrites history, invokes `gh`, starts daemon/tmux,
  or runs an agent.

## Boundaries

### Allowed Changes

- specs/e2e/p134-pr-history-bridge-helper.spec.md
- scripts/agentd_pr_history_bridge.sh
- crates/agentd-bin/tests/pr_history_bridge.rs
- docs/p0.9-deployment-checklist.md

### Forbidden

- Do not run the bridge helper against the real repository in execute mode.
- Do not rewrite local or remote history.
- Do not push to any remote from this helper.
- Do not change `scripts/agentd_open_pr.sh` behavior.
- Do not change workflow base branch policy.
- Do not modify or delete existing `.agentd/real-execute-smoke/*` evidence.

## Out of Scope

- Resolving merge conflicts automatically.
- Running the real `execute.dot` smoke after bridge creation.
- Retrying GitHub PR creation.
- Solving Claude account quota or GitHub authentication.

## Completion Criteria

<!-- lint-ack: error-path — execute opt-in and dirty-worktree scenarios are explicit failure paths. -->
<!-- lint-ack: output-mode-coverage — this helper intentionally reports through stdout/stderr only and writes no report file. -->
<!-- lint-ack: decision-coverage — the dry-run and execute scenarios bind the agentd_pr_history_bridge.sh entry point and both modes. -->

Scenario: dry-run reports bridge command without changing history
  Test:
    Package: agentd-bin
    Filter: pr_history_bridge_dry_run_reports_command_without_changing_history
  Level: script integration
  Test Double: temporary git repositories
  Given `origin/main` and local `HEAD` point to unrelated clean histories
  When `agentd_pr_history_bridge.sh` runs in default dry-run mode
  Then it exits 0
  And stdout contains `mode: dry-run`, `merge_required: yes`, and `git merge --allow-unrelated-histories --no-edit origin/main`
  And `git merge-base origin/main HEAD` still exits non-zero after the helper returns

Scenario: execute mode requires explicit opt-in
  Test:
    Package: agentd-bin
    Filter: pr_history_bridge_execute_requires_explicit_opt_in
  Level: script integration
  Test Double: temporary git repositories
  Given `origin/main` and local `HEAD` point to unrelated clean histories
  And `AGENTD_PR_HISTORY_BRIDGE` is unset
  When `agentd_pr_history_bridge.sh --execute` runs
  Then it exits non-zero
  And stderr names `AGENTD_PR_HISTORY_BRIDGE=1`

Scenario: execute mode refuses a dirty worktree before merge
  Test:
    Package: agentd-bin
    Filter: pr_history_bridge_execute_refuses_dirty_worktree
  Level: script integration
  Test Double: temporary git repositories
  Given `origin/main` and local `HEAD` point to unrelated histories
  And the worktree has an uncommitted file
  And `AGENTD_PR_HISTORY_BRIDGE=1`
  When `agentd_pr_history_bridge.sh --execute` runs
  Then it exits non-zero
  And stderr names the dirty worktree
  And `git merge-base origin/main HEAD` still exits non-zero after the helper returns

Scenario: execute mode creates a local merge base
  Test:
    Package: agentd-bin
    Filter: pr_history_bridge_execute_creates_local_merge_base
  Level: script integration
  Test Double: temporary git repositories
  Given `origin/main` and local `HEAD` point to unrelated clean histories with non-conflicting files
  And `AGENTD_PR_HISTORY_BRIDGE=1`
  When `agentd_pr_history_bridge.sh --execute` runs
  Then it exits 0
  And stdout contains `mode: execute`, `merge_required: yes`, and `merge_base`
  And `git merge-base origin/main HEAD` exits 0 after the helper returns
