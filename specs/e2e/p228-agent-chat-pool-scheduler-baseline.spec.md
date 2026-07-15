spec: task
name: "agent-chat pool scheduler baseline"
tags: [agent-chat-replacement, scheduler, pool, dispatch, phase-e, p228]
---

## Intent

Advance the agent-chat replacement path after p227 by implementing the
matrix-Agent pool scheduler baseline in agentd. Agent-chat exposes a thin
role/capability scheduler through `GET /api/pool`, `POST /api/dispatch`, and
`POST /api/dispatch/release`; agentd needs equivalent local semantics before
task graphs, workflows, dashboard views, Matrix/remote relay paths, and cutover
automation can depend on scheduler allocation. This slice makes scheduler
decisions durable and visible, but does not start or message real agents.

## Decisions

- The scheduler model mirrors agent-chat's `lib/matrix-agent.js`: roles are
  `architect`, `coding`, `testing`, `review`, `integration`, and
  `documentation`; capability tiers are `strong`, `medium`, and `lightweight`.
- Requested capability wins when it is a valid tier; otherwise the role default
  applies: `architect=strong`, `review=strong`, `coding=medium`,
  `testing=medium`, `integration=medium`, `documentation=lightweight`, and
  unknown roles default to `medium`.
- Legacy agent names infer roles when explicit `role` is absent:
  coordinator/architect => `architect`, implementer/coder/coding => `coding`,
  reviewer/final_reviewer/final-reviewer => `review`, test/qa => `testing`,
  integrat => `integration`, and doc => `documentation`.
- Agent capability is explicit when valid; otherwise it defaults from the
  effective role. `strong` may satisfy `medium` or `lightweight`; `medium` may
  satisfy `lightweight`; weaker tiers must not satisfy stronger requests.
- Scheduler state is durable in SQLite, not an in-memory set. Rebuilding the
  daemon router over the same store must preserve active reservations and queued
  tickets.
- `GET /api/pool?role=&capability=&state=idle|busy|any` returns
  `{ grid, counts, total, agents }`, where `busy=true` reflects active routed
  or drained scheduler reservations.
- `POST /api/dispatch { role, capability?, task?, room? }` requires `role`.
  If an online, not-busy eligible agent exists, it creates a durable routed
  reservation and returns `{ status:"routed", reservation, agent, role, tier }`.
- If no eligible agent exists and provision capacity is available, dispatch
  creates a durable provision reservation and returns
  `{ status:"provision", reservation, role, tier, name, runtime }`. Provision
  does not launch tmux or a real runtime; it is only a structured plan.
- If no eligible agent exists and provision capacity is unavailable, dispatch
  creates a durable queued ticket and returns
  `{ status:"queued", ticket, role, tier, queueDepth }`.
- Provision capacity is controlled by a scheduler config value equivalent to
  agent-chat's `MATRIX_AGENT_MAX_PER_CELL`. Default is `0`, so empty cells queue
  unless configured by tests or the daemon.
- `POST /api/dispatch/release { agent }` releases active routed/drained
  reservations for that agent. If a queue entry waits for the same effective
  `(role,tier)` cell, the freed agent is reserved again and the route returns
  `{ status:"drained", reservation, agent, ticket, role, tier, task, room }`.
  Otherwise it returns `{ status:"released", agent }`.
- Invalid or missing release agent returns `400`; releasing an unknown or
  already-free agent remains idempotent and returns `released`.
- Scheduler writes use existing operator bearer semantics. Pool reads stay open,
  matching agent-chat visibility.
- p228 does not change task-graph node assignment, workflow `codergen` or
  `fan_out` allocation, MCP schemas, real agent startup, Matrix/remote relay
  behavior, dashboard UI, cutover automation, rollback, or token provisioning.
- The parity map moves `pool_scheduler` from `missing` to `partial` after p228,
  while `task_graph_coordination` and `migration_shadow_cutover` remain partial
  until scheduler integration and the later replacement slices are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p228-agent-chat-pool-scheduler-baseline.spec.md
- crates/agentd-store/migrations/**
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/agent_scheduler_repo.rs
- crates/agentd-store/tests/agent_scheduler.rs
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
- Do not launch or kill agents from scheduler dispatch or release routes.
- Do not send direct messages from scheduler dispatch; the caller remains
  responsible for task delivery in this slice.
- Do not change task-graph node schemas or make task-graph assignment
  scheduler-based in p228.
- Do not change workflow engine allocation for `codergen` or `fan_out`.
- Do not implement browser/dashboard pool UI, Matrix bridge state, remote relay
  state, service cutover state, rollback plans, or token provisioning.
- Do not claim full agent-chat replacement after this slice.

## Out of Scope

- Connecting task graphs or workflow nodes to scheduler reservations.
- Cross-model reviewer selection or N-of-M gate behavior.
- Provisioned agent lifecycle management and idle reaping.
- Remote/multi-host scheduler coordination.
- Dashboard rendering of the role x tier matrix.

## Completion Criteria

<!-- lint-ack: decision-coverage - p228 binds role/tier inference, pool shape, dispatch route/queue/provision decisions, release draining, durable state, auth boundaries, migration, and docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify HTTP status codes, response bodies, persisted reservation rows after router rebuild, schema columns, and parity docs. -->

Scenario: store scheduler resolves pool roles, tiers, routing, queueing, and release draining
  Test:
    Package: agentd-store
    Filter: agent_scheduler_routes_queues_provisions_and_drains_durably
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database with online coding agents
  When the scheduler dispatches coding/medium work twice
  Then the first request routes to an idle eligible agent
  And the second request queues because that agent is busy
  When the routed agent is released
  Then the queued ticket drains onto the same agent
  And the active reservation remains visible as durable busy scheduler state

Scenario: store migration creates durable scheduler tables
  Test:
    Package: agentd-store
    Filter: migration_creates_agent_scheduler_tables
  Level: store integration
  Test Double: temp SQLite database
  Given a newly migrated agentd database
  When the schema is inspected
  Then `agent_scheduler_reservations` exists with role, tier, agent, status,
  task JSON, and timestamps
  And `agent_scheduler_queue` exists with ticket, role, tier, task JSON, room,
  status, and timestamps
  And schema version is advanced beyond p227

Scenario: HTTP pool and dispatch routes match agent-chat baseline shapes
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_pool_scheduler_routes_queue_and_release
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given an agentd HTTP router with online coding and legacy implementer agents
  When `/api/pool` is read and two coding/medium dispatches are posted
  Then the pool grid infers legacy roles and reports busy routed agents
  And dispatch responses use `routed`, `queued`, and release `drained` shapes

Scenario: HTTP scheduler supports provision plans and operator auth
  Test:
    Package: agentd-surface
    Filter: http_agent_chat_scheduler_provision_and_auth
  Level: HTTP surface integration
  Test Double: FakeRunHost
  Given hard operator bearer auth and scheduler provision capacity `1`
  When dispatch is posted without bearer auth
  Then the route returns 401
  When an empty documentation/lightweight cell is dispatched with bearer auth
  Then the first request returns a provision plan with runtime metadata
  And the second request queues because the provision cap is reached

Scenario: production daemon scheduler state persists after router rebuild
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_chat_scheduler_persists_after_router_rebuild
  Level: daemon HTTP integration
  Test Double: real SqliteStore and fake agent backend
  Given a production `ProductionRunHost` over a temp SQLite database
  When an online coding agent is registered and dispatch creates a busy
  reservation
  And the daemon router is rebuilt over the same database
  Then `/api/pool?state=busy` still reports the routed agent busy
  And releasing it returns `released`

Scenario: parity docs record p228 pool scheduler progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p228_pool_scheduler_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the scheduler row and Phase E update are inspected
  Then they mention p228 pool scheduler baseline, `/api/pool`, `/api/dispatch`,
  durable reservations, release, queue, and provision plans
  And `pool_scheduler` remains partial because task-graph/workflow integration,
  dashboard views, Matrix/remote relay, cutover, rollback, and token
  provisioning are not complete
