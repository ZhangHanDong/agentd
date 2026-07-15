spec: task
name: "agent-chat Matrix bridge backend contract baseline"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p236]
---

## Intent

Advance the agent-chat replacement path by replacing the missing Matrix bridge
row with a backend-facing contract that an external Matrix bridge process can
use. This slice persists trusted Matrix room mappings, accepts Matrix inbound
messages into the existing direct/group inbox, exposes a Matrix outbox view for
non-Matrix message wakeups, and keeps the real homeserver/SDK/puppet process
out of agentd.

## Decisions

- Add durable Matrix bridge room state keyed by Matrix `room_id`, with optional
  `group_name` or `agent_name`, `trusted`, `trust_reason`, optional
  `inviter_mxid`, and timestamps.
- Add durable Matrix bridge event state keyed by Matrix `event_id`, recording
  the `room_id`, `sender_mxid`, routed message id, route, ignored flag, and
  created timestamp so inbound events are idempotent.
- Add `POST /api/matrix/rooms` and `GET /api/matrix/rooms/:room_id` as the
  external bridge's room mapping/trust contract. A mapped group room creates or
  preserves the corresponding local group with supplied members.
- Add `POST /api/matrix/inbound` for external Matrix bridge ingress. It rejects
  unknown or untrusted rooms, ignores `[AGENTIGNORE]` messages without writing a
  message, routes group rooms to group messages with `source="matrix"`, routes
  agent rooms to direct messages, and persists direct-message Matrix metadata as
  `source="matrix"`, `sourceRoom`, `senderMxid`, and `trustLevel`.
- Add `GET /api/matrix/outbox?from_seq=N` as a polling-friendly Matrix bridge
  outbox over durable relay stream events. It returns only later `message`
  wakeups whose payload `source` is not `matrix`, preventing Matrix echo loops.
- Matrix bridge endpoints use the existing operator bearer-token boundary when
  API auth is configured. This slice does not add Matrix-specific secrets or
  token rotation.
- Keep `matrix_bridge` partial after this slice because agentd still does not
  run a real Matrix bridge process, create Matrix puppet accounts, join rooms,
  upload Matrix media, or send Matrix client-server API calls.

## Boundaries

### Allowed Changes

- specs/e2e/p236-agent-chat-matrix-bridge-contract-baseline.spec.md
- crates/agentd-store/migrations/0012_matrix_bridge_contract.sql
- crates/agentd-store/src/lib.rs
- crates/agentd-store/src/matrix_bridge_repo.rs
- crates/agentd-store/tests/migration.rs
- crates/agentd-store/tests/matrix_bridge.rs
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
- Do not add a Matrix SDK, Matrix homeserver dependency, real homeserver
  network test, real Matrix puppet registration, or Matrix media upload.
- Do not add a long-running bridge daemon, systemd/launchd service, remote
  relay process, tmux injection worker, service cutover, rollback automation,
  token provisioning, or token rotation.
- Do not claim that agentd can fully replace agent-chat after this slice.

## Out of Scope

- Matrix client-server API calls, account login/registration, room join/invite,
  puppet account lifecycle, and Matrix media transfer.
- Bot command parsing through Matrix rooms.
- Full Matrix room membership reconciliation.
- Dashboard rendering for Matrix bridge rooms or outbox state.
- Real SSE soak tests and external bridge process packaging.

## Completion Criteria

<!-- lint-ack: decision-coverage - p236 binds durable room/event state, room registration, ingress routing, ignored messages, outbox echo filtering, auth boundaries, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify SQLite rows, HTTP JSON responses/status codes, inbox metadata, outbox filtering, auth status codes, and parity Markdown without a real Matrix homeserver or bridge process. -->
<!-- lint-ack: boundary-entry-point - daemon, surface, store, and parity entry points are verified through bound package filters. -->

Scenario: store persists Matrix room mappings and inbound event records
  Test:
    Package: agentd-store
    Filter: matrix_bridge_store_persists_room_mapping_and_event_records
  Level: store integration
  Test Double: tempfile SQLite database
  Given a Matrix room mapping for room "!ops:matrix.test" to group "ops"
  When the repository stores the room and records inbound event "$event-1"
  Then the room row is trusted with reason "managed"
  And recording "$event-1" again returns the existing event without creating a second row

Scenario: migration creates Matrix bridge contract tables
  Test:
    Package: agentd-store
    Filter: migration_creates_matrix_bridge_contract_tables
  Level: store integration
  Test Double: tempfile SQLite database
  Given a freshly migrated SQLite database
  When the migration metadata and table definitions are inspected
  Then sqlite_master lists `matrix_bridge_rooms` and `matrix_bridge_events`
  And schema_meta reports version "12"

Scenario: daemon registers Matrix room mapping and creates group
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_room_registration_persists_mapping_and_group
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given the daemon HTTP API is running
  When a client posts `/api/matrix/rooms` for room "!ops:matrix.test" mapped to group "ops"
  Then `GET /api/matrix/rooms/!ops%3Amatrix.test` returns the trusted mapping
  And `GET /api/groups/ops` returns the created group with supplied members

Scenario: daemon routes Matrix inbound agent DM with metadata and idempotency
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_inbound_agent_dm_persists_source_metadata_and_dedupes_event
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given a trusted Matrix room mapped to agent "codex-worker"
  When a Matrix inbound event "$dm-1" is posted twice through `/api/matrix/inbound`
  Then the first response creates one direct message and the second response reports `duplicate=true`
  And `GET /api/inbox/codex-worker` returns one message with `source="matrix"`, `sourceRoom`, `senderMxid`, and `trustLevel`

Scenario: daemon rejects Matrix inbound from untrusted rooms
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_inbound_rejects_untrusted_room
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given no trusted Matrix room mapping exists for room "!evil:matrix.test"
  When a client posts `/api/matrix/inbound` for that room
  Then the response is HTTP 403
  And the JSON body contains `error="matrix room not trusted"`

Scenario: daemon ignores Matrix agent-ignore messages without inbox writes
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_inbound_agentignore_records_ignored_event_without_message
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given a trusted Matrix room mapped to agent "codex-worker"
  When a Matrix inbound event body starts with "[AGENTIGNORE]"
  Then the response reports `ignored=true`
  And `GET /api/inbox/codex-worker` returns no messages

Scenario: Matrix outbox filters Matrix echo events
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_outbox_filters_matrix_echo_events
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and fake backend
  Given one Matrix inbound message and one API-originated direct message have been written
  When a client gets `/api/matrix/outbox?from_seq=0`
  Then the outbox contains the API-originated message wakeup
  And the outbox does not contain the Matrix-originated message wakeup

Scenario: surface Matrix outbox replays non-Matrix message wakeups
  Test:
    Package: agentd-surface
    Filter: http_matrix_outbox_replays_non_matrix_message_wakeups
  Level: HTTP integration
  Test Double: FakeRunHost
  Given the fake host has relay stream events from "matrix" and "api"
  When a client gets `/api/matrix/outbox?from_seq=1`
  Then the JSON response includes only later non-Matrix `message` events
  And each returned event preserves its sequence id and payload

Scenario: Matrix bridge endpoints honor configured operator auth
  Test:
    Package: agentd-bin
    Filter: daemon_router_matrix_bridge_endpoints_reject_remote_operator_without_bearer
  Level: HTTP integration
  Test Double: ProductionRunHost over tempfile SQLite and hard auth config
  Given the daemon is configured with an operator bearer token
  When a remote client posts a Matrix room, posts Matrix inbound, or reads Matrix outbox without valid credentials
  Then each response is HTTP 401

Scenario: parity docs record p236 Matrix bridge progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p236_matrix_bridge_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge and remote relay rows are inspected
  Then the Matrix bridge row mentions p236 room trust, inbound ingress, and outbox progress
  And the Matrix bridge row is partial
  And the remote relay row remains partial
