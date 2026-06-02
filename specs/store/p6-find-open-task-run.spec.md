spec: task
name: "task_repo::find_open_task_run — the forward (run,node) -> task_run read"
tags: [store, p0, p0.9, task-runs]
---

## Intent

The production `RunHost` resolves an agent's `submit_outcome(run_id, node_id)`
into the `task_run_id` the engine event needs. The store today has only the
REVERSE lookup (`lookup_park_by_task_run`: task_run_id -> park). P0.9 adds the
forward read `find_open_task_run(run_id, node_id)` so `open_task` can produce the
open task run's id + worktree. This is the one non-assembly store addition for
P0.9; agentd-core stays frozen and no migration is needed (the columns exist).

## Decisions

- `task_repo::find_open_task_run(pool, run_id, node_id) -> Option<(TaskRunId, Option<String>)>` returns the OPEN task run for `(run_id, node_id)` — the one with `finished_at IS NULL` — as its id plus the nullable `worktree_path`, or `None` if there is no open task run.
- "Open" matches the park-open invariant used by `lookup_park_by_task_run` (`finished_at IS NULL`); `complete_task_run` closes it, after which the forward read returns `None` (parity with the reverse lookup).
- If more than one open row somehow exists for a `(run, node)`, the most recently started (`ORDER BY started_at DESC LIMIT 1`) is returned.
- No schema change: `task_runs` already has `run_id`, `node_id`, `worktree_path`, and `finished_at` (migration `0001_init.sql`).

## Boundaries

### Allowed Changes

- crates/agentd-store/src/task_repo.rs
- crates/agentd-store/tests/**
- specs/store/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add a database migration (the columns already exist; schema is frozen).

## Out of Scope

- Populating `agent_id` / `spec_path` / `plan_path` into `TaskAssignment` — those columns do not exist; P0.9 leaves them `None` (a known gap).

## Completion Criteria

Scenario: the open task run is found by run and node
  Test: find_open_task_run_returns_open_park
  Given a store with an inserted run and a task run for ("r1","implement")
  When find_open_task_run is called for ("r1","implement")
  Then it returns Some with that task run's id

Scenario: a completed task run is not returned
  Test: find_open_task_run_is_none_after_complete
  Given a store with a task run for ("r1","implement") that has been completed
  When find_open_task_run is called for ("r1","implement")
  Then it returns None

Scenario: an unknown node has no open task run
  Test: find_open_task_run_is_none_for_unknown_node
  Given a store with a task run for ("r1","implement")
  When find_open_task_run is called for ("r1","review")
  Then it returns None
