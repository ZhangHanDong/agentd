spec: task
name: "Production assign_task uses task_run agent ownership"
tags: [e2e, p0.9, mcp, assign-task, store]
---

## Intent

P74 defines `assign_task` as an ownership-checked tool, but the production
`RunHost` currently returns an empty `TaskAssignment.agent_id` because P6 left
`task_runs.agent_id` as a known gap. After P120, spawned agents know their
`agentd_agent_id`; this task persists that same role on the task run so
production `assign_task` can accept the owner and reject other agents.

## Decisions

- Use the existing nullable `task_runs.agent_id` column; do not add a migration.
- Add a narrow `Store::set_task_run_agent(task_run_id, agent_id)` port method
  rather than changing task-run id generation or surface tool schemas.
- `codergen` writes the role-derived `AgentId` to the task run before spawning,
  so a spawn failure cannot leave an already-launched agent with missing
  ownership.
- `task_repo::find_open_task_run` returns the open task's id, optional worktree,
  and optional agent id.
- `ProductionRunHost::open_task` maps that stored agent id into
  `TaskAssignment.agent_id`; legacy rows with a null agent id still return an
  empty string.
- `assign_task` keeps the P74 behavior unchanged: only the matching agent gets
  the task, and a different agent receives `not_assigned`.

## Boundaries

### Allowed Changes

- specs/e2e/p121-production-assign-task-agent-ownership.spec.md
- specs/store/p6-find-open-task-run.spec.md
- crates/agentd-core/src/ports/store.rs
- crates/agentd-core/src/handler/codergen.rs
- crates/agentd-core/src/test_support/in_memory_store.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-core/tests/ports_fakes.rs
- crates/agentd-store/src/task_repo.rs
- crates/agentd-store/src/store_impl.rs
- crates/agentd-store/tests/task_lookup.rs
- crates/agentd-store/tests/store_trait.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs

### Forbidden

- Do not modify crates/agentd-surface/**.
- Do not modify crates/agentd-tmux/**.
- Do not add a database migration or new dependency.
- Do not change the `assign_task` input/output JSON schema.
- Do not hand a task to an agent whose id differs from `task_runs.agent_id`.

## Out of Scope

- Persisting `spec_path`, `plan_path`, or `context_pack` in task assignments.
- Agent registry foreign keys or Matrix mxid ownership.
- Changing `submit_outcome`; it can continue resolving by `(run_id,node_id)`.
- Backfilling agent ids for historical task runs.

## Completion Criteria

Scenario: task_repo persists and returns task-run agent id
  Test: set_task_run_agent_persists_agent_id
  Level: store repository unit
  Given a SqliteStore task run with no agent id
  When `set_task_run_agent` stores agent "implementer"
  Then `find_open_task_run` returns that agent id with the task id and worktree

Scenario: codergen persists the spawned role as task owner
  Test: codergen_run_persists_agent_id_for_task_run
  Level: core handler unit
  Given a codergen node with role "implementer"
  When the handler runs and parks
  Then the task run's stored agent id is "implementer"

Scenario: production open_task returns assigned agent id
  Test: production_open_task_returns_assigned_agent_id
  Level: production RunHost integration
  Given an execute workflow run parked at implement
  When production `open_task` reads the implement assignment
  Then the returned assignment has agent_id "implementer"

Scenario: production assign_task accepts owner and rejects other agent
  Test: production_assign_task_accepts_owner_and_rejects_other_agent
  Level: production RunHost + MCP dispatch integration
  Given an execute workflow run parked at implement
  When `assign_task` is dispatched for agent "implementer"
  Then it returns the open task assignment
  And when `assign_task` is dispatched for agent "someone-else"
  Then it returns `not_assigned`

Scenario: P6 known-gap note is marked superseded
  Test: p6_spec_marks_agent_id_gap_superseded
  Level: static spec check
  Given specs/store/p6-find-open-task-run.spec.md
  When the spec is read as text
  Then it mentions that P121 supersedes the old agent_id gap
