spec: task
name: "agent-chat Matrix bridge HTTP backend and state"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p238]
---

## Intent

Advance the agent-chat replacement path by connecting the p237 Matrix bridge
runtime scaffold to the real p236 agentd HTTP contract. This slice adds a
standard-library HTTP backend client for local agentd endpoints and JSON cursor
state persistence so a future bridge process can restart without replaying
confirmed outbox sends.

## Decisions

- Add an `AgentdHttpBackend` implementation of `AgentdBridgeBackend` in
  `agentd-matrix`; it uses the existing `BridgeConfig` base URL and optional
  operator bearer token.
- Use standard-library HTTP/1.1 over `TcpStream` for this slice instead of
  adding a new HTTP client dependency. Only `http://host[:port]` agentd API
  URLs are supported.
- `AgentdHttpBackend::register_room` sends `POST /api/matrix/rooms` with the
  p236 JSON shape: `roomId`, optional `group`, optional `agent`, `trusted`,
  `trustReason`, optional `inviterMxid`, and `members`.
- `AgentdHttpBackend::post_inbound` sends `POST /api/matrix/inbound` with the
  p236 JSON shape: `eventId`, `roomId`, `senderMxid`, `body`, `mentions`, and
  optional `replyTo`.
- `AgentdHttpBackend::poll_outbox` sends
  `GET /api/matrix/outbox?from_seq=N`, parses relay `events`, preserves each
  event's raw payload, and maps `messageId`/`source`/`target` plus a body from
  `full`, `summary`, or `body` into `MatrixOutboundEvent`.
- Extend `MatrixOutboundEvent` so transports can receive optional `target` and
  raw payload metadata even when a future bridge still needs to resolve a
  target to a Matrix room. This slice does not add that reverse room-routing
  algorithm.
- Add `BridgeState` JSON load/save helpers that treat a missing state file as
  cursor `0` and persist `nextFromSeq` after successful runtime sends.
- Keep `matrix_bridge` partial after this slice because agentd still does not
  include a real Matrix SDK process, homeserver login, puppet accounts,
  join/invite handling, Matrix media transfer, Matrix bot commands, service
  cutover, rollback, token provisioning/rotation, or bridge operations.

## Boundaries

### Allowed Changes

- specs/e2e/p238-agent-chat-matrix-http-backend-state.spec.md
- crates/agentd-matrix/Cargo.toml
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bridge_runtime.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not add a real Matrix SDK adapter, Matrix homeserver dependency, real
  homeserver network test, puppet registration, Matrix media upload, long-running
  bridge daemon, systemd/launchd service, remote relay process, tmux injection
  worker, service cutover, rollback automation, token provisioning, or token
  rotation.
- Do not add a new external HTTP client dependency.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Matrix client-server API calls, account login/registration, room join/invite,
  puppet account lifecycle, Matrix media transfer, and real bridge packaging.
- Reverse target-to-Matrix-room routing for outbox events that do not yet carry a
  room id.
- Bot command parsing through Matrix rooms.
- Dashboard rendering for Matrix bridge rooms, runtime state, or outbox cursor.
- Remote relay integration, launchd/systemd installation, and operator cutover
  automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p238 binds standard-library HTTP, exact p236 room/inbound/outbox shapes, bearer auth, state persistence, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify request paths, headers, JSON bodies, response mapping, error strings, state file contents, and repository Markdown with local fake HTTP servers only. -->
<!-- lint-ack: boundary-entry-point - agentd-matrix runtime/client tests and agentctl parity artifact tests cover the listed entry points. -->

Scenario: HTTP backend posts Matrix room and inbound requests with bearer token
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_posts_room_and_inbound_with_bearer
  Level: library integration
  Test Double: local TCP fake agentd HTTP server
  Given an `AgentdHttpBackend` configured with an operator token
  When it registers one Matrix room and posts one inbound Matrix event
  Then the fake server receives `POST /api/matrix/rooms` before `POST /api/matrix/inbound`
  And both requests include `Authorization: Bearer bridge-secret`
  And both request bodies use the p236 JSON field names

Scenario: HTTP backend polls Matrix outbox and maps relay payload metadata
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_polls_outbox_and_maps_payload
  Level: library integration
  Test Double: local TCP fake agentd HTTP server
  Given the fake server returns one `/api/matrix/outbox` relay message event
  When `AgentdHttpBackend::poll_outbox` polls from cursor 7
  Then the request path is `/api/matrix/outbox?from_seq=7`
  And the returned `MatrixOutboundEvent` preserves `seq`, `messageId`, `source`, `target`, `roomId`, body, and raw payload

Scenario: HTTP backend reports non-success and invalid JSON as backend errors
  Test:
    Package: agentd-matrix
    Filter: agentd_http_backend_reports_non_success_status_and_invalid_json
  Level: library integration
  Test Double: local TCP fake agentd HTTP servers
  Given one fake server returns HTTP 500 for room registration
  When the backend registers a Matrix room
  Then it returns a backend error containing status 500
  And when another fake server returns malformed JSON for outbox polling
  Then `poll_outbox` returns a backend error containing `decode JSON`

Scenario: bridge state JSON persists cursor and defaults missing files
  Test:
    Package: agentd-matrix
    Filter: bridge_state_json_persists_cursor_and_defaults_missing
  Level: library integration
  Test Double: tempfile filesystem
  Given no bridge state file exists
  When `BridgeState::load_json` reads that path
  Then the loaded cursor is `0`
  And after saving cursor `42`, the file contains `nextFromSeq`
  And loading it again returns cursor `42`

Scenario: runtime with HTTP backend sends outbox and persists confirmed cursor
  Test:
    Package: agentd-matrix
    Filter: matrix_runtime_with_http_backend_sends_outbox_and_persists_cursor
  Level: library integration
  Test Double: local TCP fake agentd HTTP server and fake Matrix transport
  Given a fake HTTP backend returns two bridge-ready outbox events
  When `BridgeRuntime::run_once` sends both through the fake Matrix transport
  Then `BridgeState::save_json` persists cursor `2`
  And reloading the state file returns cursor `2`

Scenario: parity docs record p238 Matrix HTTP backend progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p238_matrix_http_backend_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p238, `AgentdHttpBackend`, and JSON cursor state
  And the Matrix bridge row remains partial
  And the row still names the real Matrix SDK, puppet, media, cutover, rollback, and token gaps
