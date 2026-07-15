spec: task
name: "Agent start and runtime baseline"
tags: [agent-chat-replacement, registry, lifecycle, runtime, phase-c, p214]
---

## Intent

Advance Phase C by adding the agent-chat compatible lifecycle surfaces that p213
left open: launch environment inspection, starting a registered local agent, and
recording a minimal runtime observation. This slice must make the registry more
operational without running real Claude in tests or claiming full agent-chat
replacement parity.

## Decisions

- The daemon exposes `GET /api/agents/:name/launch-env`, returning
  `{ "runtimeProfile": ... }` so agent-chat launch clients can read the stored
  runtime profile without depending on agentd's snake_case record shape.
- The daemon exposes `POST /api/agents/:name/start`, requiring an existing
  offline registered agent with a valid runtime framework and workdir.
- Valid start runtimes are Codex and Claude-compatible framework names. Tests
  must exercise Codex or fakes only; this slice must not start real Claude.
- A successful start calls the existing `AgentBackend::spawn`, marks the agent
  online, stores the returned address as `tmux_target`, clears
  `offline_reason`, updates `last_seen_at`, and returns both the updated agent
  and a serializable handle.
- Start rejects unknown agents with HTTP 404, already-online agents with HTTP
  409, missing workdirs with HTTP 400, and unsupported runtimes with HTTP 400
  before calling the backend.
- The store records a minimal `runtime_state` JSON object for
  `POST /api/agents/:name/runtime`, including blocked/activity/workspace/MCP
  observation fields and `updatedAt`; this is runtime observation state, not a
  launch profile.
- `agentctl agent launch-env`, `agentctl agent start`, and
  `agentctl agent runtime` call the same daemon endpoints through the existing
  dependency-free HTTP client.
- The parity map moves `agent_runtime_profiles` from `missing` to `partial`,
  and keeps `agent_registry_lifecycle` as `partial` until auth, import, and
  broader operational parity are complete.

## Boundaries

### Allowed Changes

- specs/e2e/p214-agent-start-runtime-baseline.spec.md
- crates/agentd-store/migrations/0004_agent_runtime_lifecycle.sql
- crates/agentd-store/src/agent_repo.rs
- crates/agentd-store/tests/agent_registry.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/migration_backcompat.rs
- crates/agentd-surface/src/host.rs
- crates/agentd-surface/src/http.rs
- crates/agentd-surface/src/test_support.rs
- crates/agentd-bin/src/host.rs
- crates/agentd-bin/tests/daemon_http.rs
- crates/agentctl/src/agent.rs
- crates/agentctl/src/cli.rs
- crates/agentctl/tests/agent_cli.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not implement API bearer tokens, per-agent tokens, bridge-secret auth, JSON
  import, shadow mode, scheduler dispatch, messaging, Matrix, remote relay, or
  dashboard agent panels.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not claim that agentd can fully replace agent-chat after this slice.
- Do not add new Cargo dependencies for the `agentctl` HTTP client.

## Out of Scope

- Killing, reconnecting, or re-binding runtime sessions.
- Native non-tmux runtime storage beyond the existing `AgentBackend::spawn`
  seam.
- Full agent-chat runtime state machine and blocked-notification fanout.
- Importing existing agent-chat JSON stores.

## Completion Criteria

<!-- lint-ack: decision-coverage - the bound store, daemon, CLI, and parity tests cover this narrow runtime lifecycle baseline. -->
<!-- lint-ack: observable-decision-coverage - this slice binds HTTP JSON output, CLI request paths, persisted SQLite state, and backend spawn requests. -->
<!-- lint-ack: boundary-entry-point - daemon and CLI entry points are verified through the bound `agentd-bin` and `agentctl` package filters that live in the listed test files. -->

Scenario: store marks started agents online
  Test:
    Package: agentd-store
    Filter: agent_registry_start_marks_agent_online_and_records_runtime_state
  Level: store integration
  Test Double: tempfile SQLite database
  Given an offline registered Codex agent with a workdir and runtime profile
  When the store marks the agent started with a runtime handle address
  Then the agent status becomes "online"
  And the stored tmux target is the handle address
  And a later runtime update persists blocked, activity, workspace, and MCP
  observation fields

Scenario: runtime lifecycle migration preserves existing agents
  Test:
    Package: agentd-store
    Filter: agent_runtime_lifecycle_migration_preserves_existing_agents
  Level: migration backcompat
  Test Double: raw in-memory SQLite pool
  Given a database migrated through p213 with an existing agent
  When migration 0004 is applied
  Then the existing agent remains readable
  And the new `runtime_state` column defaults to an empty JSON object

Scenario: daemon returns launch-env runtime profile
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_launch_env_returns_runtime_profile
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given a registered agent with a runtime profile
  When a client gets `/api/agents/codex-sec/launch-env`
  Then the response is HTTP 200
  And the JSON body contains the stored profile under `runtimeProfile`

Scenario: daemon starts Codex agents through backend spawn
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_start_spawns_codex_and_marks_online
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given an offline registered Codex agent with a workdir
  When a client posts `/api/agents/codex-worker/start`
  Then the fake backend sees one Codex spawn request for that workdir
  And the response reports the agent online with a tmux target
  And no real Claude process is started

Scenario: daemon rejects invalid start requests before spawn
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_start_rejects_unknown_online_missing_workdir_and_bad_runtime
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given a daemon router over an empty store
  When clients attempt to start an unknown agent, an already-online agent, an
  offline agent without a workdir, and an offline agent with an unsupported
  runtime
  Then the responses are HTTP 404, 409, 400, and 400 respectively
  And the fake backend sees no spawn request for rejected starts

Scenario: daemon records runtime observations
  Test:
    Package: agentd-bin
    Filter: daemon_router_agent_runtime_update_records_observation
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake ports
  Given a registered agent
  When a client posts `/api/agents/codex-worker/runtime` with blocked,
  activity, workspace, and MCP fields
  Then the response returns the persisted runtime state
  And the agent remains inspectable with the same runtime state

Scenario: agentctl calls launch-env start and runtime endpoints
  Test:
    Package: agentctl
    Filter: agent_cli_launch_env_start_and_runtime_use_api_agents
  Level: CLI
  Test Double: one-shot local TCP daemon
  Given a local fake HTTP daemon that records requests
  When `agentctl agent launch-env`, `agentctl agent start`, and
  `agentctl agent runtime` run against it
  Then the fake daemon observes `/api/agents/:name/launch-env`,
  `/api/agents/:name/start`, and `/api/agents/:name/runtime`
  And the CLI exits successfully and prints the daemon response body

Scenario: parity map records p214 runtime lifecycle progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p214_runtime_lifecycle_progress
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the agent-chat replacement parity map
  When the `agent_registry_lifecycle` and `agent_runtime_profiles` rows are
  inspected
  Then both rows remain "partial"
  And their decisions mention p214 launch-env, start, runtime observation, and
  runtime profile progress without claiming full replacement coverage
