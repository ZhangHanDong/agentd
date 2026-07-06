spec: task
name: "Open PR preflight helper"
tags: [e2e, workflow, pr, github, safety]
---

## Intent

Make `open_pr` fail early with an actionable local error when the published task
branch cannot become a GitHub pull request. The real execute smoke reached
`gh pr create` only to discover that `agentd/${task_run_id}` had no common
history with `origin/main`; the workflow should preflight that condition before
delegating to `gh`.

## Decisions

- Add one argv-safe helper: `scripts/agentd_open_pr.sh ${task_run_id}`.
- The helper validates `task_run_id`, fetches `origin/main` into the local
  remote-tracking ref, checks that `agentd/${task_run_id}` exists locally and on
  `origin`, and verifies that it has a merge-base with `origin/main`.
- When histories have no common ancestor, the helper exits non-zero before
  invoking `gh` and stderr names both compared refs.
- On success, the helper invokes `gh pr create --fill --base main --head
  agentd/${task_run_id}` so the base branch is explicit.
- PR workflows call the helper from `open_pr` instead of embedding `gh pr
  create` directly in DOT.

## Boundaries

### Allowed Changes

- specs/e2e/p130-open-pr-preflight.spec.md
- specs/e2e/p100-worktree-pr-publication.spec.md
- specs/e2e/p102-pr-workflow-worktree-publication.spec.md
- specs/e2e/p127-execute-context-report-readiness.spec.md
- specs/e2e/p98-per-run-serialization.spec.md
- specs/core/p11-per-task-run-worktree.spec.md
- specs/workflow/p81-execute-dot.spec.md
- specs/workflow/p84-builtin-workflows.spec.md
- workflows/execute.dot
- workflows/docs-only.dot
- workflows/bugfix-rapid.dot
- workflows/refactor-only.dot
- scripts/agentd_open_pr.sh
- crates/agentd-bin/tests/open_pr.rs
- crates/agentctl/tests/workflows.rs
- crates/agentd-bin/tests/contract.rs

### Forbidden

- Do not change publish_branch semantics in this slice.
- Do not add schema columns for base branch or PR URL.
- Do not shell-chain commands inside DOT `cmd=` values.
- Do not attempt to rewrite local or remote git history.

## Out of Scope

- Repairing the current remote history mismatch automatically.
- Choosing a non-main base branch policy.
- Creating PR titles or bodies beyond `gh --fill`.
- Supplying GitHub authentication tokens.

## Completion Criteria

Scenario: helper rejects a published branch with no common base before gh
  Test:
    Package: agentd-bin
    Filter: open_pr_rejects_no_common_history_before_gh
  Level: script integration
  Test Double: temporary git repositories and fake gh
  Given `origin/main` and `agentd/tr_0123456789ABCDEFGHJKMNPQRS` point to unrelated histories
  When `agentd_open_pr.sh` runs for that task id
  Then it exits non-zero
  And stderr names the missing common history between the task branch and `origin/main`
  And the fake `gh` executable is not called

Scenario: helper fetches base and delegates to gh for compatible history
  Test:
    Package: agentd-bin
    Filter: open_pr_invokes_gh_with_explicit_base_and_head
  Level: script integration
  Test Double: temporary git repository and fake gh
  Given `origin/main` and the published task branch share history
  When `agentd_open_pr.sh` runs for that task id
  Then it exits 0
  And `gh` receives `pr create --fill --base main --head agentd/tr_0123456789ABCDEFGHJKMNPQRS`

Scenario: execute.dot uses the open PR helper
  Test:
    Package: agentctl
    Filter: execute_dot_publishes_worktree_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/execute.dot
  When it is parsed and validated
  Then `open_pr` shells `bash scripts/agentd_open_pr.sh ${task_run_id}`
  And publication edges remain success-conditioned

Scenario: migrated PR workflows use the open PR helper
  Test:
    Package: agentctl
    Filter: docs_only_dot_publishes_worktree_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given docs-only.dot, bugfix-rapid.dot, and refactor-only.dot
  When their PR topology assertions run
  Then each `open_pr` shells `bash scripts/agentd_open_pr.sh ${task_run_id}`

Scenario: ProductionRunHost records the helper between publish and report
  Test:
    Package: agentd-bin
    Filter: production_runhost_execute_publishes_worktree_branch_before_pr
  Level: e2e contract
  Test Double: FakeBackend + RecordingCommandRunner + real SqliteStore + fake WorktreeAllocator
  Given `crates/agentd-bin/tests/contract.rs` verifies ProductionRunHost executes `execute.dot`
  When publish_branch succeeds and open_pr runs
  Then the recorded open_pr command is `bash scripts/agentd_open_pr.sh ${task_run_id}`
  And it still runs after publish_branch and before the acceptance report
