spec: task
name: "Preserve active worktrees on daemon boot-GC"
tags: [e2e, store, tmux, daemon, p2, worktree, recovery]
---

## Intent

P99 activates task-run worktrees, P101 releases the implementer worktree only
after a successful terminal workflow, and P104 gives reviewers independent
snapshot worktrees. The old boot-GC rule, "remove every pool worktree on daemon
startup", no longer matches the lifecycle model: an in-flight or failed run can
still have durable checkpoint/context state pointing at a pool-owned worktree.

This slice makes daemon boot-GC preserve pool worktrees that are still referenced
by non-finished runs, while continuing to remove unreferenced leftovers and
successful-run cleanup debris.

## Decisions

- The production daemon asks the store for active worktree paths before running
  boot-GC.
- Active implementer worktrees are task-run `worktree_path` values whose parent
  run is not `finished`. This intentionally includes `failed` runs because P101
  keeps failed worktrees for debugging and manual recovery.
- Active reviewer worktrees are `review_worktrees.worktree_path` values that
  have not been taken/released and whose parent run is not `finished`.
- `WorktreePool` removes only listed pool-owned paths whose basename is not in
  the active preserve set. Basename comparison keeps preservation robust when
  git reports canonicalized paths such as `/private/tmp/...` while the store
  contains `/tmp/...`.
- Finished runs are not preserved. Their worktrees should have been released on
  success; if release failed, boot-GC remains fallback cleanup.
- The existing `gc_on_boot()` behavior remains available as
  `gc_on_boot_preserving([])` for callers that have no durable store context.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- crates/agentd-store/**
- crates/agentd-tmux/**
- specs/e2e/**

### Forbidden

- Do not preserve every pool worktree unconditionally.
- Do not preserve finished successful runs' worktrees.
- Do not use `task_runs.finished_at IS NULL` as the implementer preservation
  rule; downstream verify/review/publish nodes can still need the implementer
  worktree after the task run itself is complete.
- Do not match loose human worktree names such as `wt-task-feature` or
  `wt-review-feature`.
- Do not add durable release retry queues in this slice.

## Out of Scope

- Re-spawning agents automatically on daemon restart.
- Multi-codergen workflows with more than one implementer worktree.
- Manual cleanup commands for failed-run debug worktrees.
- Persistent telemetry for boot-GC decisions.

## Completion Criteria

Scenario: store lists active implementer and reviewer worktrees
  Test: active_worktree_paths_include_non_finished_task_and_review_worktrees
  Level: store integration
  Test Double: real SqliteStore on tempfile
  Given task and reviewer worktrees for running, failed, and finished runs
  When active_worktree_paths is read
  Then it includes task worktrees for running and failed runs
  And it includes unreleased reviewer worktrees for non-finished runs
  And it excludes finished-run worktrees and already released reviewer worktrees

Scenario: boot-GC preserves active paths by pool basename
  Test: boot_gc_preserves_active_worktrees_by_pool_basename
  Level: adapter unit
  Test Double: in-memory WorktreeProvider
  Given existing pool worktrees reported under canonicalized paths
  And active store paths using equivalent basenames under different parent paths
  When gc_on_boot_preserving runs
  Then the active task and reviewer worktrees remain
  And unreferenced pool worktrees are removed

Scenario: production daemon wires store active paths into boot-GC
  Test: build_production_host_preserves_active_worktrees_during_boot_gc
  Level: daemon assembly integration
  Test Double: real SqliteStore on tempfile + in-memory WorktreeProvider
  Given a persisted non-finished run with a task worktree and an unreferenced pool worktree
  When the production boot-GC helper runs
  Then it preserves the persisted active worktree
  And it removes the unreferenced pool worktree
