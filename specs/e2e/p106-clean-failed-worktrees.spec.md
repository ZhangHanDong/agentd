spec: task
name: "Manual cleanup for failed-run worktrees"
tags: [e2e, store, tmux, daemon, cli, p2, worktree, cleanup]
---

## Intent

P101 deliberately keeps failed runs' worktrees for debugging and recovery, and
P105 makes boot-GC preserve those paths across daemon restarts. That is correct
for recovery, but it also means failed-run worktrees can accumulate forever
unless an operator has an explicit cleanup path after the debug value is gone.

This slice adds a controlled manual cleanup path for failed-run worktrees. It is
dry-run by default, removes only worktrees tied to failed runs, and clears the
store reference only after the pool release succeeds.

## Decisions

- Add a daemon/bin cleanup helper that reads failed-run worktree cleanup
  candidates from the store and releases them through the existing
  `WorktreeAllocator::release` path.
- Add an offline `agentd cleanup-worktrees` subcommand. It reuses the normal
  `--db-path`, `--repo-dir`, and `--worktree-base` options and defaults to
  dry-run; `--execute` is required to delete anything.
- Cleanup candidates are:
  - task-run worktrees where the parent run status is `failed`;
  - reviewer snapshot worktrees where the parent run status is `failed` and
    `review_worktrees.released_at IS NULL`.
- Running/parked runs are never cleanup candidates; P105's restart preservation
  remains intact.
- Finished runs are not cleanup candidates here; successful cleanup remains the
  normal terminal release plus boot-GC fallback path.
- Store references are cleared after a successful release: task worktrees clear
  `task_runs.worktree_path`, and reviewer snapshot worktrees set
  `review_worktrees.released_at`.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- crates/agentd-store/**
- specs/e2e/**

### Forbidden

- Do not delete worktrees by raw filesystem removal; route through the pool
  release validation so key/path mismatches cannot delete foreign paths.
- Do not clean running or parked runs.
- Do not clean finished successful runs in this command.
- Do not clear the store reference if release fails.
- Do not make cleanup execute by default.

## Out of Scope

- Durable release retry queues.
- Automatic cleanup by age.
- Cleaning non-pool human worktrees.
- Clearing stale DB references whose underlying worktree is already missing.

## Completion Criteria

Scenario: store lists only failed-run cleanup candidates
  Test: failed_worktree_cleanup_candidates_include_only_failed_runs
  Level: store integration
  Test Double: real SqliteStore on tempfile
  Given task and reviewer worktrees for running, failed, and finished runs
  When failed_worktree_cleanup_candidates is read
  Then it includes only the failed run's task and unreleased reviewer worktrees
  And it excludes running, finished, and already released reviewer worktrees

Scenario: dry-run cleanup is non-destructive
  Test: cleanup_failed_worktrees_dry_run_lists_without_releasing
  Level: daemon assembly integration
  Test Double: real SqliteStore on tempfile + in-memory WorktreeProvider
  Given a failed run with task and reviewer worktrees and an unrelated running worktree
  When cleanup_failed_worktrees runs with execute=false
  Then it reports the failed-run candidates
  And it does not remove any provider worktree
  And it does not clear active store references

Scenario: execute cleanup removes only failed-run worktrees and clears references
  Test: cleanup_failed_worktrees_execute_removes_failed_worktrees_and_clears_refs
  Level: daemon assembly integration
  Test Double: real SqliteStore on tempfile + in-memory WorktreeProvider
  Given a failed run with task and reviewer worktrees and an unrelated running worktree
  When cleanup_failed_worktrees runs with execute=true
  Then the failed-run pool worktrees are released
  And the running worktree remains
  And active_worktree_paths excludes the released failed-run paths

Scenario: CLI exposes cleanup as dry-run by default
  <!-- lint-ack: testability — CleanupWorktrees is an exact enum variant name asserted by the parser test. -->
  Test: agentd_cli_cleanup_worktrees_is_dry_run_by_default
  Level: CLI unit
  Test Double: clap parser
  Given the argv ["agentd", "cleanup-worktrees"]
  When the CLI parses it
  Then cmd variant equals CleanupWorktrees
  And execute is false
