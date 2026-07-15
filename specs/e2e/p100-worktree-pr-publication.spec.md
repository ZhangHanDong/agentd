spec: task
name: "Worktree PR publication - publish allocated worktree branch before open_pr"
tags: [e2e, workflow, p2, worktree, pr]
---

## Intent

P11/R3a and P99/R3b1 make `execute.dot` implement, verify, and review the
allocated per-`task_run` worktree, but the workflow still reaches `open_pr` with
no guarantee that the detached worktree has been committed, pushed, or named as
a PR head branch. This slice closes that publication gap for `execute.dot`
only: after the review fan-in passes, the workflow publishes the allocated
worktree to a deterministic task branch, then opens the PR against that branch.

The branch name is derived from the existing `task_run_id` context value:
`agentd/${task_run_id}`. This keeps the branch identity explicit and replayable
without adding schema. Tool nodes still run in the daemon cwd; code location is
passed as an argv value, preserving the R3b1 `.agentd/run/*` runtime-state
boundary.

## Decisions

- Add a `publish_branch` tool node between `aggregate` and `open_pr` in
  `workflows/execute.dot`.
- The publish node shells one argv-safe helper:
  `bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}`.
- `scripts/agentd_publish_worktree.sh` validates the `task_run_id`, switches the
  allocated worktree to `agentd/${task_run_id}`, stages all changes, commits only
  when staged changes exist, and pushes `HEAD:agentd/${task_run_id}`.
- `open_pr` invokes `bash scripts/agentd_open_pr.sh ${task_run_id}` so PR
  creation targets the published task branch through the preflight helper.
- The `aggregate -> publish_branch`, `publish_branch -> open_pr`, and
  `open_pr -> report_acceptance` edges are success-conditioned. A failed publish
  or failed PR creation must not fall through to the acceptance report.

## Boundaries

### Allowed Changes

- workflows/execute.dot
- scripts/agentd_publish_worktree.sh
- crates/agentctl/tests/**
- crates/agentd-bin/tests/**
- specs/e2e/**
- specs/workflow/p81-execute-dot.spec.md

### Forbidden

- Do not make all tool nodes run with cwd set to the worktree.
- Do not add `task_runs.branch_name` or any other schema column in this slice.
- Do not migrate `bugfix-rapid.dot`, `refactor-only.dot`, or `docs-only.dot`.
- Do not require a shell chain in DOT `cmd=` values; keep one program plus argv.

## Out of Scope

- Choosing base branch or PR title/body policy beyond the open PR helper.
- Auth/token provisioning for `gh`.
- Release or cleanup of the allocated worktree after the PR is opened is handled
  by P101.
- Persisting `head_commit`, `diff_sha256`, or branch metadata to `task_runs`.

## Completion Criteria

Scenario: execute.dot declares the publication bridge before open_pr
  Test: execute_dot_publishes_worktree_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/execute.dot
  When it is parsed and validated
  Then the aggregate-to-publish, publish-to-open_pr, and open_pr-to-report edges are success-conditioned
  And the publish_branch command is "bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}"
  And the open_pr command is "bash scripts/agentd_open_pr.sh ${task_run_id}"

Scenario: execute.dot walks through publish then opens the task branch PR
  Test: execute_dot_walks_to_done
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given execute.dot on the real Engine with a fake allocator returning "/tmp/agentd-task-wt"
  When implement succeeds, reviewers pass, and all tool nodes succeed
  Then the run reaches Finished
  And the publish command receives "/tmp/agentd-task-wt" and the actual task_run_id
  And open_pr shells "bash scripts/agentd_open_pr.sh ${task_run_id}"

Scenario: publish failure stops before open_pr
  Test: execute_dot_publish_failure_stops_before_open_pr
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given execute.dot on the real Engine and a publish_branch tool result with exit status 1
  When implement succeeds and reviewers pass
  Then the run fails without recording the open PR helper command

Scenario: open_pr failure stops before report_acceptance
  Test: execute_dot_open_pr_failure_stops_before_report_acceptance
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given execute.dot on the real Engine and an open_pr tool result with exit status 1
  When implement succeeds, reviewers pass, and publish_branch succeeds
  Then the run fails without recording the report_acceptance command

Scenario: ProductionRunHost publishes before opening PR
  Test: production_runhost_execute_tools_use_stable_repo_cwd_after_review_fan_in
  Level: e2e contract
  Test Double: FakeBackend + RecordingCommandRunner + real SqliteStore + fake WorktreeAllocator
  Given a ProductionRunHost with a fake WorktreeAllocator returning "/tmp/agentd-task-wt"
  When an execute.dot run completes through passing reviewers
  Then the runner records publish_branch before open_pr
  And open_pr receives `${task_run_id}`
