spec: task
name: "agent-chat task-graph scheduler integration"
tags: [agent-chat-replacement, task-graph, scheduler, dispatch, phase-e, p229]
---

## Intent

Advance the agent-chat replacement path after p228 by making agentd task graphs
the first internal caller of the durable pool scheduler. A task-graph node may
request a role/capability cell instead of naming a fixed assignee; ready nodes
then reserve an eligible agent, deliver the dispatch message to that agent, and
release the reservation when the node completes or fails. This closes the
task-graph/scheduler gap without launching real runtimes or changing workflow
`codergen`/`fan_out` allocation yet.

## Decisions

- Existing task-graph nodes with only `assignee` keep p227 behavior: they are
  dispatched directly to that assignee and do not create scheduler reservations.
- Scheduled nodes use optional `role` and `capability` fields. `role` enables
  scheduler dispatch; `assignee` is optional for scheduled nodes and is filled
  with the routed agent name once a reservation is created.
- A node is valid when it has a non-empty `assignee` or a non-empty scheduler
  `role`. Nodes with neither are rejected.
- Scheduler dispatch for a ready node uses the p228 role/tier rules and stores
  task metadata containing `graphId`, `nodeId`, `description`, and dependency
  results so queued tickets can later be drained back into the graph.
- When scheduler dispatch returns `routed`, the node becomes `dispatched`, the
  node stores `schedulerReservationId`, `schedulerStatus`, `role`, `tier`, and
  the chosen `assignee`, and the durable direct message schema includes the same
  scheduler metadata.
- When scheduler dispatch returns `queued`, the node remains not delivered, stores
  `schedulerTicket`, `schedulerStatus="queued"`, `role`, and `tier`, and later
  advancement must not enqueue a duplicate ticket for the same node.
- When scheduler dispatch returns `provision`, the node remains not delivered,
  stores provision metadata, and waits for a later provisioning slice; no real
  tmux, Codex, Claude, Matrix, or relay process is started.
- When a scheduled node reaches `complete` or `failed`, agentd releases the
  scheduler reservation for its assignee. If release drains a queued task-graph
  ticket, agentd immediately marks that queued node `dispatched`, assigns the
  freed agent, stores the drained reservation id, and writes the dispatch direct
  message.
- Result messages still require the sender to match the node's current assignee
  and `reply_to` to match the node dispatch message id.
- p229 updates the parity map and roadmap to show task-graph scheduler
  integration progress, while `pool_scheduler`, `task_graph_coordination`, and
  `migration_shadow_cutover` remain partial until workflow allocation,
  dashboard views, Matrix/remote relay, cutover, rollback, and token
  provisioning are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p229-agent-chat-task-graph-scheduler-integration.spec.md
- crates/agentd-store/src/agent_chat_task_graph_repo.rs
- crates/agentd-store/tests/agent_chat_task_graphs.rs
- crates/agentd-surface/src/host.rs
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
- Do not launch, kill, or provision real agents from task-graph advancement.
- Do not change the existing direct-assignee task-graph dispatch behavior for
  nodes that do not declare scheduler `role`.
- Do not change workflow engine `codergen`, reviewer `fan_out`, or run graph
  allocation in this slice.
- Do not implement browser/dashboard queue UI, Matrix bridge state, remote relay
  state, service cutover state, rollback plans, or token provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Scheduler allocation for workflow `codergen` and `fan_out` nodes.
- Provisioned agent startup and idle reaping.
- Cross-host or remote relay scheduler coordination.
- Dashboard rendering of queued/dispatched graph nodes.
- Import-time token provisioning or token rotation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p229 binds scheduled node fields, route/queue/drain behavior, legacy assignee compatibility, result-message auth, production durability, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify SQLite-backed graph rows, direct inbox messages, scheduler reservation state, daemon HTTP bodies, and parity Markdown. -->

Scenario: store task graphs route scheduled nodes and drain queued graph tickets
  Test:
    Package: agentd-store
    Filter: agent_chat_task_graph_scheduler_routes_queues_and_drains_nodes
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database with one online coding agent
  When a task graph contains two dependency-ready coding/medium nodes
  Then the first scheduled node reserves the agent and receives a durable direct
  dispatch message with scheduler metadata
  And the second scheduled node records one queued scheduler ticket without a
  duplicate dispatch message
  When the first node completes through the task-graph result path
  Then the reservation is released
  And the queued ticket drains onto the freed agent and dispatches the second
  graph node exactly once

Scenario: direct-assignee task graphs remain scheduler-free
  Test:
    Package: agentd-store
    Filter: agent_chat_task_graph_direct_assignee_nodes_do_not_create_scheduler_reservations
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database
  When a task graph node declares only `assignee`
  Then the node dispatches to that assignee as before
  And no scheduler reservation or queue ticket is created for that graph

Scenario: production daemon routes scheduled task-graph nodes through durable scheduler state
  Test:
    Package: agentd-bin
    Filter: daemon_router_task_graph_scheduler_routes_and_releases_nodes
  Level: daemon HTTP integration
  Test Double: real SqliteStore and fake agent backend
  Given a production daemon router with one online coding/medium agent
  When an operator creates a graph with two root nodes requesting
  `role="coding"` and `capability="medium"`
  Then `/api/pool?state=busy` reports the chosen agent busy
  And the first node's inbox dispatch schema includes `schedulerReservationId`
  When the first node posts a valid `task_graph_result`
  Then the second queued node is drained to the same agent and receives one
  dispatch message

Scenario: scheduled task-graph nodes keep result-message sender and reply binding
  Test:
    Package: agentd-bin
    Filter: daemon_router_task_graph_scheduler_rejects_spoofed_result_messages
  Level: daemon HTTP integration
  Test Double: real SqliteStore and fake agent backend
  Given a production daemon router with one scheduled dispatched node assigned
  by the scheduler
  When a different agent posts a `task_graph_result` for that graph/node
  Then the response has `taskGraph=null`
  And the scheduler reservation remains busy
  When the assigned agent posts the same result without `reply_to`
  Then the response has `taskGraph=null`
  And the graph node remains dispatched

Scenario: parity docs record p229 task-graph scheduler progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p229_task_graph_scheduler_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the task graph, pool scheduler, migration, and Phase E sections are
  inspected
  Then they mention p229 task-graph scheduler integration, scheduled nodes,
  reservation metadata, queued ticket drain, and result-time release
  And they remain partial because workflow allocation, dashboard views,
  Matrix/remote relay, cutover, rollback, and token provisioning are not
  complete
