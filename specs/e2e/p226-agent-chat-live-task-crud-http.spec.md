spec: task
name: "agent-chat live task CRUD HTTP parity"
tags: [agent-chat-replacement, tasks, http, crud, phase-d, p226]
---

## Intent

Advance the agent-chat replacement path after p225 by making agentd's imported
agent-chat task snapshots live through agent-chat-compatible `/api/tasks` HTTP
CRUD endpoints. This lets operators and assigned agents create, inspect, update,
transition, comment on, and delete product-level coordination tasks in agentd
without routing them into workflow `task_runs`. Task-graph DAG execution,
scheduler dispatch, dashboard task pages, Matrix/remote relay state, service
cutover, rollback automation, and token provisioning remain separate slices.

## Decisions

- The live task surface uses the agent-chat-compatible endpoints:
  `POST /api/tasks`, `GET /api/tasks`, `GET /api/tasks/:id`,
  `PATCH /api/tasks/:id`, `PATCH /api/tasks/:id/execution`,
  `DELETE /api/tasks/:id`, `POST /api/tasks/:id/accept`,
  `POST /api/tasks/:id/transition`, `POST /api/tasks/:id/comments`, and
  `GET /api/agents/:name/tasks`.
- Live tasks are stored in the p225 `agent_chat_tasks` compatibility table so
  imported `tasks.json` rows become editable agentd rows after cutover.
- The workflow engine task table remains separate; p226 does not read or write
  workflow `task_runs`.
- Task fields and defaults follow agent-chat's task store: required trimmed
  `title`, default `description=""`, `status="created"`, `priority="p2"`,
  `granularity="task"`, nullable `assignee`, `created_by`, `parent_id`,
  ISO-like timestamp strings, nullable execution metadata, deduplicated labels,
  nullable `health`, and bounded comments.
- Listing supports agent-chat filters `assignee`, comma-separated `status`,
  `priority`, `label`, plus non-negative `offset` and positive capped `limit`.
- Operator writes (`POST`, `PATCH`, `DELETE`, and `comments`) use the existing
  configured bearer semantics. Reads stay open, matching agent-chat.
- Agent-owned writes (`execution`, `accept`, and `transition`) authorize through
  the task assignee using the existing agent token hard/audit semantics.
- Lifecycle transitions match agent-chat:
  `created -> accepted -> in_progress -> blocked -> in_progress -> done`, with
  `in_progress -> done` also allowed and `blocked -> done` rejected.
- Blocking requires both `waiting_reason` and `waiting_until`; resuming to
  `in_progress` and finishing as `done` clear waiting metadata.
- `/api/agents/:name/tasks` returns only tasks assigned to that normalized
  agent name.
- The parity map remains partial after p226 until task-graph DAG dispatch,
  scheduler integration, dashboard views, Matrix/remote relay state, service
  cutover, rollback, and token provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p226-agent-chat-live-task-crud-http.spec.md
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/agent_chat_task_repo.rs
- crates/agentd-store/tests/agent_chat_tasks.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, tmux, systemd, launchd, or remote relay in
  automated tests.
- Do not route live product tasks into workflow `task_runs`.
- Do not implement `/api/task-graphs`, DAG advancement, scheduler dispatch,
  dashboard task pages, Matrix bridge state, remote relay state, service
  cutover state, rollback plans, or token provisioning in this slice.
- Do not change MCP tool schemas, runtime launch behavior, workflow engine
  behavior, or existing agent/message/group endpoint semantics.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Live task-graph CRUD and DAG node advancement.
- Assigning tasks through a scheduler or dispatching task nodes to agent pools.
- Supervisor health enrichment beyond preserving stored `health` JSON.
- Browser/dashboard task management UI.
- Service cutover and rollback automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p226 binds task field defaults, filters, lifecycle transitions, comments, deletion, auth boundaries, durable production host storage, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify HTTP status codes, response bodies, persisted rows after router rebuild, auth rejection, invalid transition rejection, and docs. -->

Scenario: store live task CRUD preserves agent-chat task semantics
  Test:
    Package: agentd-store
    Filter: agent_chat_live_task_repo_crud_filters_transitions_and_comments
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database
  When a live agent-chat task is created, listed with filters, patched,
  execution-updated, accepted, transitioned, commented on, and deleted
  Then the stored task follows agent-chat defaults and lifecycle rules
  And comments and labels are serialized in compatibility columns
  And the deleted task is no longer returned

Scenario: HTTP task CRUD matches agent-chat response shapes
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_crud_filters_comments_and_delete
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given the agentd HTTP router over a fake host
  When an operator creates tasks, lists them with filters and pagination,
  patches a task, adds a comment, and deletes it
  Then writes return `{ ok: true, task }`
  And reads return a task object or task array in the agent-chat-compatible shape
  And missing task reads return 404 with `task not found`

Scenario: HTTP task lifecycle enforces agent-chat transitions
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_lifecycle_rejects_invalid_transitions
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given a created task assigned to `codex-a`
  When the task is accepted, started, blocked with waiting metadata, resumed,
  and completed
  Then status, started/completed timestamps, and waiting fields match
  agent-chat lifecycle behavior
  And created-to-in_progress, blocked-to-done, and blocked-without-waiting
  requests return 400 without mutating the task

Scenario: HTTP agent task routes use assignee token auth when configured
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_agent_routes_require_assignee_token
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given hard agent token mode with a token for `codex-a`
  And a task assigned to `codex-a`
  When the assigned agent route is called without `X-Agent-Token`
  Then the route returns 403
  When the same route is called with the configured token
  Then the transition succeeds

Scenario: production daemon task CRUD persists through store-backed host
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_chat_task_crud_persists_after_router_rebuild
  Level: daemon HTTP integration
  Test Double: real SqliteStore and fake agent backend
  Given a production `ProductionRunHost` over a temp SQLite database
  When a task is created, transitioned, and commented through HTTP
  And the daemon router is rebuilt over the same database
  Then the task remains visible with its status, comment, and assignment

Scenario: parity docs record p226 live task CRUD progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p226_live_task_crud_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the task graph and migration rows and Phase D update are inspected
  Then they mention p226 live `/api/tasks` CRUD parity
  And they remain partial because task-graph DAG dispatch, scheduler
  integration, dashboard views, Matrix/remote relay state, service cutover,
  rollback, and token provisioning are not complete
