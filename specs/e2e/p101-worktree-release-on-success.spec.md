spec: task
name: "Release allocated worktree after successful workflow completion"
tags: [e2e, workflow, p2, worktree, cleanup]
---

## Intent

P99 activates per-`task_run` worktree allocation and P100 publishes the allocated
worktree branch before opening a PR. The remaining lifecycle gap is cleanup:
task-keyed worktrees are removed only by boot-GC, so successful runs leak a
pool-owned worktree until the next daemon restart.

This slice releases the allocated worktree after the workflow successfully
reaches its terminal node. It deliberately does not release at
`complete_task_run`: the implementer task is completed before downstream
`verify_lifecycle`, reviewer fan-out, `publish_branch`, and `open_pr` have
consumed the implementation tree. After P104, reviewers consume independent
snapshots and release those snapshots on verdict; this slice still owns the
implementer worktree release at workflow terminal. It also deliberately keeps
failed runs' worktrees for debugging and possible manual recovery.

## Decisions

- Extend the core `WorktreeAllocator` port with `release(key, path)`.
- `Engine` attempts release only on a successful terminal transition, using the
  staged `task_run_id` and `worktree` context values.
- Release is best-effort after the run is marked Finished. A release error is
  logged and leaves the run Finished; boot-GC remains the fallback cleanup.
- `WorktreePool::release` removes only the exact task-keyed pool path derived
  from `task_run_id`; a mismatched path is rejected so release cannot delete a
  foreign worktree.
- Failed runs do not release the worktree in this slice.

## Boundaries

### Allowed Changes

- crates/agentd-core/**
- crates/agentd-tmux/**
- crates/agentd-bin/tests/**
- specs/e2e/**
- specs/core/p11-per-task-run-worktree.spec.md

### Forbidden

- Do not release the worktree from `codergen.resume` / `complete_task_run`.
- Do not release failed runs' worktrees.
- Do not delete a path whose basename does not match `wt-task-${task_run_id}`.
- Do not add schema or persist release state in this slice.

## Out of Scope

- Release retries or durable cleanup queues.
- Multi-codergen workflows with more than one allocated worktree in context.
- Manual cleanup commands.
- Removing the boot-GC fallback.

## Completion Criteria

Scenario: Engine releases the allocated worktree after successful terminal completion
  Test: engine_releases_allocated_worktree_after_successful_terminal
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + recording WorktreeAllocator
  Given a graph that allocates a worktree at codergen and then runs a downstream tool before done
  When the agent outcome is submitted and the downstream tool succeeds
  Then the run reaches Finished
  And the allocator release is called with the same task_run_id and worktree path

Scenario: Engine does not release on workflow failure
  Test: engine_does_not_release_allocated_worktree_on_failure
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + recording WorktreeAllocator
  Given a graph that allocates a worktree and then a downstream tool exits 1
  When the agent outcome is submitted
  Then the run fails
  And the allocator release is not called

Scenario: WorktreePool releases only the matching task-keyed worktree
  Test: pool_releases_task_keyed_worktree_via_allocator_port
  Level: adapter unit
  Test Double: in-memory WorktreeProvider
  Given a WorktreePool that allocated task_run_id "tr_0123456789ABCDEFGHJKMNPQRS"
  When the core WorktreeAllocator port releases that same path for that same task_run_id
  Then the provider removes that worktree

Scenario: WorktreePool rejects mismatched release paths
  Test: pool_release_rejects_mismatched_task_keyed_path
  Level: adapter unit
  Test Double: in-memory WorktreeProvider
  Given a WorktreePool and task_run_id "tr_0123456789ABCDEFGHJKMNPQRS"
  When release is requested for "/wt/wt-task-feature"
  Then release returns an error and the provider does not remove the foreign path
