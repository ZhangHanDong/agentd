spec: task
name: "agent-chat remote relay backend compatibility baseline"
tags: [agent-chat-replacement, remote-relay, matrix-remote, phase-g, p235]
---

## Intent

Advance the agent-chat replacement path by adding the first backend-facing
remote relay compatibility slice in agentd. This slice gives existing or future
relay adapters a durable server heartbeat surface, delivery-event audit trail,
and message wakeup stream without implementing a real remote package, Matrix
puppet bridge, or tmux injection worker inside agentd.

## Decisions

- Add a durable `relay_servers` table keyed by server id. A heartbeat records
  `instance_id`, `boot_ts`, advertised `agents`, advertised `sessions`,
  `last_seen_at`, `heartbeat_at`, `online`, `maintenance`, and `agent_count`.
- Add `POST /api/servers/heartbeat` as an operator/relay endpoint protected by
  the existing bearer-token operator path when API auth is configured. Missing
  or blank server ids return HTTP 400 with `server required`.
- A heartbeat marks listed agents online on that server, creates missing agent
  records as Codex-compatible agent records, records their tmux targets when a
  matching session is advertised, and marks previously-online agents on the
  same server offline with reason `heartbeat-missing:<server>`.
- Add a durable `delivery_events` table for relay audit events. Events keep the
  relay/source `type`, optional `message_id`, `queue_entry_id`, `agent`,
  `target`, `reason`, `source`, and arbitrary context JSON.
- Add `POST /api/delivery-events` for relay audit writes and
  `GET /api/agents/:name/delivery-events` for bounded per-agent inspection.
  Agent delivery-event reads use the same agent token boundary as other
  agent-owned read surfaces when hard token mode is configured.
- Add `GET /api/stream` as an agent-chat-compatible SSE wakeup stream for
  message creation. It replays durable direct and group message events as
  `event: message` frames and supports a `from_seq` cursor.
- Message writes through `/api/messages` append a relay wakeup event so remote
  relay adapters do not depend on run-specific SSE endpoints.
- Keep the Matrix bridge row missing after this slice. Keep the remote relay row
  partial because this slice does not package, install, run, or verify a remote
  relay process and does not perform tmux injection.

## Boundaries

### Allowed Changes

- specs/e2e/p235-agent-chat-remote-relay-backend-baseline.spec.md
- crates/agentd-store/migrations/0011_remote_relay_backend.sql
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/relay_repo.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/remote_relay.rs
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

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not add a real Matrix homeserver dependency, real remote host dependency,
  real tmux injection worker, systemd/launchd service, or remote package.
- Do not implement Matrix puppet registration, Matrix room trust policy, remote
  auto-update, dashboard queue controls, service cutover, rollback automation,
  token provisioning, or token rotation.
- Do not claim that agentd can fully replace agent-chat after this slice.

## Out of Scope

- Matrix bridge inbound/outbound room routing.
- Remote relay binary packaging, install verification, and auto-update.
- Real SSE long-running network soak tests.
- Relay-side idle gate, blocked detection, and tmux pane injection.
- Server maintenance/offline endpoints beyond heartbeat-created state.
- Dashboard rendering for relay servers or delivery events.

## Completion Criteria

<!-- lint-ack: decision-coverage - p235 binds durable server heartbeat, heartbeat error handling, delivery-event audit, stream replay, auth boundaries, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify SQLite rows, HTTP JSON responses, SSE frame data, auth status codes, and parity Markdown without real Matrix, remote host, tmux injection, or Claude execution. -->
<!-- lint-ack: boundary-entry-point - daemon, surface, store, and parity entry points are verified through bound package filters. -->

Scenario: store persists relay servers and delivery events
  Test:
    Package: agentd-store
    Filter: remote_relay_store_persists_server_heartbeats_and_delivery_events
  Level: store integration
  Test Double: tempfile SQLite database
  Given a relay heartbeat for server "remote-host-1" with two agents and two sessions
  When the relay repository records the heartbeat and appends a delivery event
  Then the relay server row reports `online=true`, `agent_count=2`, and the advertised sessions
  And the delivery event can be listed by agent name with its context JSON intact

Scenario: migration creates remote relay backend tables
  Test:
    Package: agentd-store
    Filter: migration_creates_remote_relay_backend_tables
  Level: store integration
  Test Double: tempfile SQLite database
  Given a freshly migrated SQLite database
  When the migration metadata and table definitions are inspected
  Then sqlite_master lists `relay_servers`, `delivery_events`, and `relay_stream_events`
  And schema_meta reports version "11"

Scenario: daemon heartbeat marks advertised remote agents online
  Test:
    Package: agentd-bin
    Filter: daemon_router_remote_server_heartbeat_marks_agents_online_and_missing_offline
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given one online agent already assigned to server "remote-host-1"
  When a client posts `/api/servers/heartbeat` for "remote-host-1" advertising only a different agent
  Then the advertised agent is present, online, assigned to "remote-host-1", and has its tmux target
  And the previously online missing agent is offline with reason `heartbeat-missing:remote-host-1`

Scenario: daemon heartbeat rejects missing server ids
  Test:
    Package: agentd-bin
    Filter: daemon_router_remote_server_heartbeat_rejects_missing_server
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given the daemon HTTP API is running
  When clients post `/api/servers/heartbeat` with a missing, empty, or blank `server`
  Then each response is HTTP 400
  And the JSON body contains `error="server required"`

Scenario: daemon records and exposes delivery events
  Test:
    Package: agentd-bin
    Filter: daemon_router_delivery_event_records_and_lists_agent_events
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given a registered Codex agent named "codex-worker"
  When a relay posts `/api/delivery-events` for that agent
  Then the response persists a delivery event id and `ok=true`
  And `GET /api/agents/codex-worker/delivery-events?limit=5` returns the event with its context JSON

Scenario: stream replays message wakeup events
  Test:
    Package: agentd-surface
    Filter: http_stream_replays_message_wakeup_events
  Level: HTTP integration
  Test Double: FakeRunHost
  Given the fake host has direct message wakeup events with increasing sequence ids
  When a client gets `/api/stream?from_seq=1`
  Then the SSE body contains only later `event: message` frames
  And each frame data includes the message id and target agent

Scenario: daemon message writes append wakeup stream events
  Test:
    Package: agentd-bin
    Filter: daemon_router_message_write_appends_relay_stream_event
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given the daemon HTTP API is running
  When a client writes a direct message through `/api/messages`
  Then a later `GET /api/stream` response contains an `event: message` frame
  And the frame data includes the written message id and target agent

Scenario: remote relay endpoints honor configured auth boundaries
  Test:
    Package: agentd-bin
    Filter: daemon_router_remote_relay_endpoints_reject_remote_operator_without_bearer
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and hard auth config
  Given the daemon is configured with an operator bearer token and hard agent tokens
  When a remote client posts heartbeat, posts a delivery event, or reads delivery events without valid credentials
  Then operator writes return HTTP 401
  And agent delivery-event reads reject missing or incorrect agent tokens

Scenario: parity docs record p235 remote relay progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p235_remote_relay_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the remote relay and Matrix bridge rows are inspected
  Then the remote relay row mentions p235 heartbeat, delivery-event audit, and message wakeup stream progress
  And the remote relay row remains partial
  And the Matrix bridge row remains missing
