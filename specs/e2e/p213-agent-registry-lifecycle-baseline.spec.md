spec: task
name: "Agent registry lifecycle baseline"
tags: [agent-chat-replacement, registry, lifecycle, phase-c, p213]
---

## Intent

Begin Phase C of the agent-chat replacement roadmap by turning agentd's early
`agents` table into a durable local registry surface. This slice must represent
agent-chat style agent identity fields and expose register, list, inspect,
heartbeat, and offline state through the daemon and `agentctl`, while leaving
process start, runtime updates, auth, and import for later Phase C specs.

## Decisions

- The daemon exposes agent-chat compatible local endpoints under `/api/agents`:
  `POST /api/agents`, `GET /api/agents`, `GET /api/agents/:name`,
  `POST /api/agents/:name/heartbeat`, and `POST /api/agents/:name/offline`.
- The stored agent identity model includes at least `name`, `role`,
  `capability`, `runtime`, `model`, `tmux_target`, `home_dir`, `workdir`,
  `state_dir`, `server`, `status`, `offline_reason`, `last_seen_at`,
  `registered_at`, `updated_at`, and `runtime_profile`.
- `POST /api/agents` upserts by normalized `name`; a request with a non-empty
  `tmux_target` marks the agent `online`, otherwise the agent is persisted as
  `offline`.
- `POST /api/agents/:name/heartbeat` creates a missing agent, marks it
  `online`, updates `last_seen_at`, preserves compatible `tmux_target` and
  workspace metadata, and returns whether the record was created.
- `POST /api/agents/:name/offline` marks an existing agent `offline`, stores a
  reason that defaults to `manual-offline`, and clears `tmux_target` unless the
  request sets `clear_tmux` to `false`.
- `agentctl agent ls`, `agentctl agent inspect`, `agentctl agent register`,
  `agentctl agent heartbeat`, and `agentctl agent offline` call the daemon
  over the same dependency-free HTTP style already used by `agentctl run start`.
- This slice updates the parity map to show concrete Phase C progress, but
  `agent_registry_lifecycle` remains `partial` until start and runtime update
  surfaces exist; `agent_runtime_profiles` remains `missing`.

## Boundaries

### Allowed Changes

- specs/e2e/p213-agent-registry-lifecycle-baseline.spec.md
- crates/agentd-store/migrations/0003_agent_registry_lifecycle.sql
- crates/agentd-store/src/agent_repo.rs
- crates/agentd-store/src/lib.rs
- crates/agentd-store/tests/agent_registry.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-surface/tests/http.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/src/agent.rs
- crates/agentctl/src/cli.rs
- crates/agentctl/src/main.rs
- crates/agentctl/tests/agent_cli.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not implement agent process start, supervisor launch, runtime update,
  Matrix, messaging, scheduler, JSON import, shadow mode, or cutover.
- Do not claim that agentd can fully replace agent-chat after this slice.
- Do not add new Cargo dependencies for the `agentctl` HTTP client.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.

## Out of Scope

- `POST /api/agents/:name/start` and launch environment generation.
- `POST /api/agents/:name/runtime` blocked/activity updates.
- API bearer tokens, per-agent tokens, and bridge-secret authorization.
- Agent-chat JSON import and migration shadow mode.
- Dashboard agent panels.

## Completion Criteria

<!-- lint-ack: decision-coverage - the bound store, daemon, CLI, and parity tests cover the listed registry decisions for this narrow Phase C baseline. -->
<!-- lint-ack: observable-decision-coverage - this slice binds HTTP JSON output, CLI stdout/stderr, and persisted SQLite state. -->
<!-- lint-ack: boundary-entry-point - daemon and CLI entry points are verified through the bound `agentd-bin` and `agentctl` package filters that live in the listed test files. -->

Scenario: store persists agent-chat identity fields
  Test:
    Package: agentd-store
    Filter: agent_registry_registers_lists_and_inspects_agent_chat_identity_fields
  Level: store integration
  Test Double: tempfile SQLite database
  Given an empty migrated agentd store
  When an agent named "codex-sec" is registered with role, capability, runtime,
  model, tmux target, home directory, workdir, state dir, server, and runtime
  profile metadata
  Then listing agents returns exactly that agent
  And inspecting "codex-sec" returns the same identity fields
  And the persisted status is "online"

Scenario: store heartbeat and offline update liveness
  Test:
    Package: agentd-store
    Filter: agent_registry_heartbeat_and_offline_update_liveness_state
  Level: store integration
  Test Double: tempfile SQLite database
  Given an empty migrated agentd store
  When "codex-worker" sends a heartbeat with server, tmux target, and workspace
  path
  Then the store creates the agent with status "online"
  And a later offline request with reason "manual-offline" changes status to
  "offline"
  And the offline transition clears the tmux target by default

Scenario: store rejects empty agent names
  Test:
    Package: agentd-store
    Filter: agent_registry_rejects_empty_agent_name
  Level: store integration
  Test Double: tempfile SQLite database
  Given an empty migrated agentd store
  When a register request uses an empty agent name
  Then the request is rejected before inserting a row
  And listing agents remains empty

Scenario: daemon registers lists and inspects agents
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_registry_round_trips_register_list_inspect
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given an agentd daemon router over an empty store
  When a client posts `/api/agents` for "codex-sec"
  Then the response is HTTP 200 with `{ "ok": true, "agent": ... }`
  And `GET /api/agents` includes "codex-sec"
  And `GET /api/agents/codex-sec` returns the registered runtime and path fields

Scenario: daemon heartbeat creates and offline clears liveness
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_heartbeat_creates_and_offline_clears_tmux
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given an agentd daemon router over an empty store
  When a client posts `/api/agents/codex-worker/heartbeat`
  Then the response reports `created` as true and the agent status as "online"
  When the client posts `/api/agents/codex-worker/offline`
  Then the response reports status "offline" and `tmux_target` is null

Scenario: daemon returns 404 for unknown inspect and offline
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_unknown_inspect_and_offline_return_404
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given an agentd daemon router over an empty store
  When a client gets `/api/agents/ghost`
  Then the response status is HTTP 404
  When a client posts `/api/agents/ghost/offline`
  Then the response status is HTTP 404

Scenario: agentctl calls daemon agent endpoints
  Test:
    Package: agentctl
    Filter: agent_cli_register_ls_inspect_heartbeat_and_offline_use_api_agents
  Level: CLI
  Test Double: one-shot local TCP daemon
  Given a local fake HTTP daemon that records requests
  When `agentctl agent register`, `agentctl agent ls`, `agentctl agent inspect`,
  `agentctl agent heartbeat`, and `agentctl agent offline` run against it
  Then the fake daemon observes `/api/agents` and `/api/agents/:name` requests
  And the CLI exits successfully and prints the daemon response body

Scenario: parity map records partial Phase C progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p213_registry_lifecycle_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `agent_registry_lifecycle` row is inspected
  Then its status remains "partial"
  And its decision mentions p213 register, list, inspect, heartbeat, and offline
  And it does not claim start or runtime update coverage
