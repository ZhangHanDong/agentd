spec: task
name: "TaskAssignment returns runtime metadata from checkpoint context"
tags: [e2e, surface, daemon, task-assignment, real-agent]
---

## Intent

P121 made production `assign_task` return the owning `agent_id`, and P127/P128
made real execute prompts carry `spec_path` and `plan_path` through run context.
The `TaskAssignment` and `assign_task` output schema already have
`spec_path`, `plan_path`, and `context_pack` fields, but production `open_task`
still returns `None` for them. This slice closes that task-assignment metadata
gap by reading the current checkpoint context for the open task's run.

## Decisions

- `ProductionRunHost::open_task` keeps using `task_repo::find_open_task_run` for
  `task_run_id`, `worktree`, and `agent_id`.
- `ProductionRunHost::open_task` loads the current checkpoint for the same run
  and maps top-level string context keys `spec_path`, `plan_path`, and
  `context_pack` into `TaskAssignment`.
- Missing checkpoints, absent keys, and non-string values are treated as `None`
  for those optional fields; they are not assignment errors.
- Do not change the public `assign_task` input or output JSON shape; this only
  populates fields that already exist.
- Update the deployment checklist so it no longer lists `spec_path` and
  `plan_path` as remaining `TaskAssignment` gaps after P136.

## Boundaries

### Allowed Changes

- specs/e2e/p136-task-assignment-runtime-metadata.spec.md
- specs/e2e/p125-deployment-checklist-p121-gap-accuracy.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/contract.rs
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not add schema columns for `spec_path`, `plan_path`, or `context_pack`.
- Do not change MCP tool names, `assign_task` input JSON, or output field names.
- Do not make `assign_task` fail when runtime metadata is absent.
- Do not move runtime-state files into task worktrees or change tool-node cwd.

## Out of Scope

- Seeding initial workflow context in `start_workflow`.
- Persisting PR metadata or acceptance reports in the database.
- Creating a new context-pack file format.
- Changing reviewer prompt contents or MCP stdio payload shapes.

## Completion Criteria

Scenario: production open_task returns checkpoint runtime metadata
  Test:
    Package: agentd-bin
    Filter: production_open_task_returns_checkpoint_runtime_metadata
  Level: daemon contract
  Test Double: real SqliteStore on tempfile
  Given an open task run and a checkpoint with string `spec_path`, `plan_path`, and `context_pack`
  When production `open_task` reads that assignment
  Then the returned `TaskAssignment` includes those three metadata values
  And it still includes the stored task owner and worktree

Scenario: production open_task ignores absent or non-string runtime metadata
  Test:
    Package: agentd-bin
    Filter: production_open_task_ignores_non_string_runtime_metadata
  Level: daemon contract
  Test Double: real SqliteStore on tempfile
  Given an open task run and a checkpoint where runtime metadata is absent or non-string
  When production `open_task` reads that assignment
  Then the optional metadata fields are `None`
  And the assignment still resolves successfully

Scenario: deployment checklist marks TaskAssignment metadata gap closed
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p136_task_assignment_metadata_resolved
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md
  When the TaskAssignment known-gap text is inspected
  Then it names P136 as the runtime metadata bridge
  And it does not list `spec_path` or `plan_path` as remaining TaskAssignment gaps
