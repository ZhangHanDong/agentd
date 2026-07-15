spec: task
name: "agent-chat live task-graph HTTP parity"
tags: [agent-chat-replacement, task-graph, http, dispatch, phase-d, p227]
---

## Intent

Advance agent-chat replacement after p226 by making imported task-graph
snapshots live in agentd. Operators need `/api/task-graphs` parity to create,
list, inspect, and cancel task graphs; assigned agents need node update parity;
and graph advancement must dispatch dependency-ready nodes through agentd's
durable direct-message inbox so existing multi-agent coordination can move from
agent-chat to agentd. Scheduler allocation, dashboard task-graph pages,
Matrix/remote relay delivery, service cutover, rollback automation, and token
provisioning remain separate slices.

## Decisions

- The live task-graph surface uses the agent-chat-compatible endpoints:
  `POST /api/task-graphs`, `GET /api/task-graphs`,
  `GET /api/task-graphs/:id`, `DELETE /api/task-graphs/:id`, and
  `PATCH /api/task-graphs/:id/nodes/:nodeId`.
- Live graphs reuse the p225 `agent_chat_task_graphs` compatibility table, with
  `raw_json` as the canonical graph body and indexed `owner`, `label`, and
  `status` columns. Imported graph snapshots therefore become live graph rows
  after cutover.
- `direct_messages` gains `schema_json` so task-graph dispatch and result
  messages can carry agent-chat-style `schema.kind` metadata through the same
  durable inbox as ordinary direct messages.
- Graph validation follows agent-chat: graph `owner` and `label` are required;
  each node requires an id, `assignee`, and `description`; dependencies must
  reference existing nodes; self-dependencies and cycles are rejected.
- Graph statuses are `active`, `complete`, `failed`, and `cancelled`. Node
  statuses are `pending`, `dispatched`, `active`, `complete`, `failed`,
  `skipped`, and `cancelled`.
- `POST /api/task-graphs` persists the graph, advances it immediately, and
  dispatches dependency-ready root nodes as durable direct messages with
  `schema.kind="task_graph_dispatch"`.
- Dispatch messages are idempotent by deterministic message id and store their
  `message_id` on the graph node; retrying advancement must not duplicate
  messages.
- `PATCH /api/task-graphs/:id/nodes/:nodeId` updates only `status`, `result`,
  and `error`, then advances the graph. Terminal dependency failures cascade to
  pending descendants; all-terminal graphs become `complete` unless any node
  failed, in which case the graph becomes `failed`.
- Conditional nodes support `condition.dep`, `condition.path` or `field`,
  `eq`, `neq`, `in`, and `op/value` forms. A false condition skips the node;
  unsafe path segments `__proto__`, `constructor`, and `prototype` are ignored.
- `DELETE /api/task-graphs/:id` cancels the graph and every non-terminal node
  instead of removing the row, matching agent-chat operator semantics.
- Assigned-agent node updates authorize through the node assignee using the
  existing `X-Agent-Token` hard/audit semantics. Operator graph create/delete
  routes use the existing bearer semantics. Reads stay open.
- Direct `POST /api/messages` handles `schema.kind="task_graph_result"` and
  `schema.kind="task_graph_failed"` when the message replies to that node's
  dispatch `message_id` and the sender matches the node assignee. The response
  includes `taskGraph` with handled graph/node status, or `null` when ignored.
- p227 does not introduce scheduler-based assignment; node `assignee` remains
  the dispatch target from the request or imported graph.
- The parity map remains partial after p227 until scheduler integration,
  dashboard task-graph views, Matrix/remote relay state, service cutover,
  rollback, and token provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p227-agent-chat-live-task-graph-http.spec.md
- crates/agentd-store/migrations/**
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/message_repo.rs
- crates/agentd-store/src/agent_chat_import.rs
- crates/agentd-store/src/agent_chat_task_graph_repo.rs
- crates/agentd-store/tests/agent_chat_task_graphs.rs
- crates/agentd-store/tests/messages.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not modify the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout or any source JSON file.
- Do not run real Claude, Matrix, tmux, systemd, launchd, or remote relay in
  automated tests.
- Do not route live task graphs into workflow `task_runs`.
- Do not implement scheduler allocation, pool reservations, dashboard
  task-graph pages, Matrix bridge state, remote relay state, service cutover
  state, rollback plans, or token provisioning in this slice.
- Do not change workflow engine behavior, runtime launch behavior, or MCP tool
  schemas unrelated to exposing task-graph message metadata.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Scheduler selection of node assignees.
- Browser/dashboard task-graph management UI.
- Matrix and remote relay delivery of graph dispatch/result messages.
- Import-time token provisioning or token rotation.
- Service cutover and rollback automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p227 binds graph validation, CRUD routes, dispatch, node advancement, result hook, auth boundaries, schema persistence, production durability, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify HTTP bodies/statuses, durable messages, graph rows after router rebuild, auth rejection, invalid graph rejection, and docs. -->

Scenario: store task-graph repo validates graphs, dispatches roots, advances dependencies, and cancels
  Test:
    Package: agentd-store
    Filter: agent_chat_task_graph_repo_dispatch_advance_conditions_and_cancel
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database
  When a chain task graph with a conditional descendant is created and advanced
  Then dependency-ready root nodes are dispatched as durable direct messages
  And completing dependencies dispatches or skips downstream nodes deterministically
  And deleting the graph marks the graph and non-terminal nodes cancelled

Scenario: store direct messages preserve task-graph schema metadata
  Test:
    Package: agentd-store
    Filter: direct_messages_round_trip_schema_metadata_for_task_graphs
  Level: store integration
  Test Double: temp SQLite database
  Given a direct message with `schema.kind="task_graph_result"`
  When it is inserted and read from the recipient inbox
  Then the schema metadata is preserved in the returned direct message record

Scenario: HTTP task-graph routes match agent-chat CRUD and node update shapes
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_graph_crud_dispatch_and_node_updates
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given the agentd HTTP router over a fake host
  When an operator creates a graph, lists it, reads it by id, completes a node,
  and deletes it
  Then responses use `{ ok: true, graph }` or `{ ok: true, graph, node }`
  And dispatched/completed/cancelled statuses match agent-chat task-graph
  behavior

Scenario: HTTP task-graph routes reject invalid graphs and enforce assignee token auth
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_graph_rejects_invalid_graphs_and_requires_assignee_token
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given hard agent token mode with a token for node assignee `codex-a`
  When a cyclic graph is created
  Then the route returns 400 with a dependency-cycle error
  When an assigned node update is called without `X-Agent-Token`
  Then the route returns 403
  When the same update is called with the configured token
  Then the node update succeeds

Scenario: direct task-graph result messages complete assigned nodes
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_task_graph_result_messages_complete_assigned_nodes
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given a graph with a dispatched node assigned to `codex-a`
  When `codex-a` posts a direct `task_graph_result` message replying to the
  dispatch message id
  Then the message response includes `taskGraph.handled=true`
  And the graph node becomes `complete`
  When a non-assignee posts the same result or the reply binding is missing
  Then `taskGraph` is `null` and the graph is unchanged

Scenario: production daemon task-graph state and dispatch messages persist
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_chat_task_graphs_persist_after_router_rebuild
  Level: daemon HTTP integration
  Test Double: real SqliteStore and fake agent backend
  Given a production `ProductionRunHost` over a temp SQLite database
  When a graph is created, root dispatch is read from the assignee inbox, and a
  node is completed through HTTP
  And the daemon router is rebuilt over the same database
  Then the graph remains visible with its node statuses and dispatch message id

Scenario: parity docs record p227 live task-graph progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p227_live_task_graph_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the task graph and migration rows and Phase D update are inspected
  Then they mention p227 live `/api/task-graphs` parity and dispatch messages
  And they remain partial because scheduler integration, dashboard views,
  Matrix/remote relay state, service cutover, rollback, and token provisioning
  are not complete
