spec: task
name: "agent-chat task JSON import and shadow audit"
tags: [agent-chat-replacement, migration, shadow, tasks, task-graph, phase-d, phase-h, p225]
---

## Intent

Advance the agent-chat replacement cutover path after p224 by importing
agent-chat `data/tasks.json` and optional `data/task_graphs.json` state into
agentd compatibility tables. This preserves product-level task and task-graph
state for migration and shadow audits without mixing it into workflow
`task_runs`, while leaving live task CRUD, DAG dispatch semantics, Matrix state,
remote relay state, service cutover, rollback automation, and token
provisioning for later slices.

## Decisions

- Operator entry points are `agentctl parity import-tasks` and
  `agentctl parity shadow-tasks`.
- `agentctl parity import-tasks --agent-chat <path> --db-path <path>` is
  dry-run by default: it validates the checkout, reads `tasks.json` and
  optional `task_graphs.json`, reports planned task and task-graph counts, and
  does not create or mutate the target SQLite database.
- `agentctl parity import-tasks --agent-chat <path> --db-path <path>
  --execute` opens and migrates the target SQLite database, then upserts
  supported task and task-graph snapshots into agent-chat compatibility tables.
- `agentctl parity shadow-tasks --agent-chat <path> --db-path <path>` is
  read-only: it compares supported task ids and task-graph ids from agent-chat
  JSON against the target database and exits 0 only when none are missing.
- Task snapshots preserve `id`, `title`, `description`, `status`, `priority`,
  `granularity`, `assignee`, `created_by`, `created_at`, `updated_at`,
  `started_at`, `completed_at`, `heartbeat_at`, `waiting_reason`,
  `waiting_until`, `parent_id`, `labels`, `health`, and `comments` by storing
  the full source JSON plus indexed compatibility columns.
- Task graph snapshots preserve each graph's source id and full JSON from
  `task_graphs.json`; p225 does not execute DAG advancement or dispatch.
- The importer is additive and non-destructive: it never writes to the
  agent-chat checkout and never removes target rows absent from the source.
- Malformed supported task/task-graph JSON rejects execute mode without partial
  task or task-graph writes.
- The parity map keeps `task_graph_coordination` and `migration_shadow_cutover`
  partial until live task CRUD, DAG dispatch, scheduler integration, dashboard
  views, Matrix/remote relay state, service cutover, rollback automation, and
  token provisioning are done.

## Boundaries

### Allowed Changes

- specs/e2e/p225-agent-chat-task-import-shadow.spec.md
- crates/agentctl/src/cli.rs
- crates/agentctl/src/parity.rs
- crates/agentctl/tests/parity_cli.rs
- crates/agentd-store/migrations/**
- crates/agentd-store/src/agent_chat_import.rs
- crates/agentd-store/tests/agent_chat_import.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, tmux, systemd, launchd, or remote relay in
  automated tests.
- Do not route imported tasks into workflow `task_runs`.
- Do not implement live task CRUD, task-graph DAG advancement, scheduler
  dispatch, dashboard UI, Matrix bridge state, remote relay state, service
  cutover state, rollback plans, or token provisioning in this slice.
- Do not change daemon HTTP routes, MCP tool schemas, runtime launch behavior,
  or workflow engine behavior.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Live `/api/tasks` and `/api/task-graphs` parity.
- Scheduling imported tasks or dispatching imported task-graph nodes.
- Matrix bridge state, remote relay state, alert state, runtime sessions, and
  delivery-event history.
- Browser/dashboard import UI.
- Service cutover and rollback automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p225 binds CLI dry-run/execute/audit, schema creation, store import, atomic error handling, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify stdout, exit codes, db existence, persisted rows, source checkout non-mutation, and docs. -->

Scenario: task import dry-run reports a plan without opening the database
  Test:
    Package: agentctl
    Filter: parity_task_import_dry_run_reports_counts_without_creating_db
  Level: CLI integration
  Test Double: temp agent-chat fixture and temp db path
  Given an agent-chat fixture with two tasks and one task graph
  When `agentctl parity import-tasks --agent-chat <path> --db-path <db>` runs
  Then stdout reports dry-run mode and planned task/task-graph counts
  And the target database file does not exist
  And the source `tasks.json` and `task_graphs.json` files are unchanged

Scenario: task import execute writes task and task-graph snapshots
  Test:
    Package: agentctl
    Filter: parity_task_import_execute_writes_tasks_and_graphs
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given an empty target SQLite database path
  When `agentctl parity import-tasks --agent-chat <path> --db-path <db>
  --execute` runs
  Then exit status is 0
  And stdout reports imported task and task-graph counts
  And a follow-up `shadow-tasks` audit exits 0
  And the target database contains the imported task id, status, assignee, raw
  JSON, task-graph id, and task-graph raw JSON

Scenario: task shadow audit reports drift without mutating
  Test:
    Package: agentctl
    Filter: parity_task_shadow_audit_reports_missing_tasks_without_mutating
  Level: CLI integration
  Test Double: temp agent-chat fixture and real SqliteStore
  Given a target SQLite database containing only one of two source tasks and no
  source task graph
  When `agentctl parity shadow-tasks --agent-chat <path> --db-path <db>` runs
  Then exit status is 1
  And stdout names the missing task id and missing task graph id
  And a second audit reports the same missing ids

Scenario: store task importer rejects malformed tasks atomically
  Test:
    Package: agentd-store
    Filter: agent_chat_task_import_rejects_malformed_tasks_without_partial_writes
  Level: store integration
  Test Double: temp malformed agent-chat fixture and real SqliteStore
  Given source `tasks.json` is malformed
  When the store task importer runs with execute mode
  Then the import returns an error
  And no agent-chat task or task-graph compatibility rows are written to the
  target database

Scenario: migration creates agent-chat task compatibility tables
  Test:
    Package: agentd-store
    Filter: migration_creates_agent_chat_task_import_tables
  Level: migration integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database
  When SQLite tables and schema version are inspected
  Then `agent_chat_tasks` and `agent_chat_task_graphs` exist
  And schema version is "8"

Scenario: parity map records p225 task import shadow progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p225_task_import_shadow_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the task graph and migration rows are inspected
  Then they mention p225 task import, task-graph snapshot preservation, and
  shadow audit
  And they remain partial because live task CRUD, DAG dispatch, scheduler
  integration, dashboard views, Matrix/remote relay state, service cutover,
  rollback, and token provisioning are not complete
