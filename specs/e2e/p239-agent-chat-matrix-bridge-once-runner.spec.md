spec: task
name: "agent-chat Matrix bridge one-shot runner"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p239]
---

## Intent

Advance the agent-chat replacement path by making the p237/p238 Matrix bridge
scaffold runnable as a deterministic one-shot process. This slice adds a
file-backed Matrix transport and an `agentd matrix-bridge-once` command so the
bridge loop can register rooms, forward inbound events, poll agentd outbox,
resolve outbound targets to Matrix rooms, append sent Matrix messages to a
JSONL log, and persist the confirmed cursor without a real Matrix homeserver.

## Decisions

- Add a file-backed Matrix transport in `agentd-matrix` that loads room
  registrations and inbound events from JSON fixture files, then appends sent
  outbound events to a JSONL file.
- Add a small room directory that resolves outbound Matrix rooms by first using
  `MatrixOutboundEvent.room_id`, then falling back to a trusted room whose
  `agent_name` or `group_name` matches `MatrixOutboundEvent.target`.
- Treat unmapped outbound targets as transport errors and do not advance the
  bridge cursor when the send fails.
- Add a one-shot runner function that composes `AgentdHttpBackend`,
  `FileMatrixTransport`, `BridgeState::load_json`, `BridgeRuntime::run_once`,
  and `BridgeState::save_json`.
- Add `agentd matrix-bridge-once` as an operator command with explicit paths for
  state, room registrations, inbound events, and sent JSONL output. The command
  reuses the existing global `--api-token` / `AGENTD_API_TOKEN` operator auth.
- Keep fixture JSON in the current `agentd-matrix` Rust struct shape for this
  slice. Compatibility with agent-chat's Matrix SDK event stream is out of
  scope until a real Matrix adapter exists.
- Keep `matrix_bridge` partial after this slice because agentd still does not
  include a real Matrix SDK process, homeserver login, puppet accounts,
  join/invite handling, Matrix media transfer, Matrix bot commands,
  long-running bridge service packaging, service cutover, rollback, token
  provisioning/rotation, or bridge operations.

## Boundaries

### Allowed Changes

- specs/e2e/p239-agent-chat-matrix-bridge-once-runner.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/bridge_runtime.rs
- crates/agentd-matrix/tests/http_backend.rs
- crates/agentd-matrix/tests/file_transport.rs
- crates/agentd-bin/Cargo.toml
- crates/agentd-bin/src/cli.rs
- crates/agentd-bin/src/lib.rs
- crates/agentd-bin/src/main.rs
- crates/agentd-bin/src/matrix_bridge.rs
- crates/agentd-bin/tests/matrix_bridge_once.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not add a real Matrix SDK adapter, Matrix homeserver dependency, real
  homeserver network test, puppet registration, Matrix media upload,
  long-running bridge daemon, systemd/launchd service, remote relay process,
  tmux injection worker, service cutover, rollback automation, token
  provisioning, or token rotation.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Matrix client-server API calls, account login/registration, room join/invite,
  puppet account lifecycle, Matrix media transfer, and real bridge packaging.
- Matrix bot command parsing.
- Dashboard rendering for Matrix bridge rooms, runtime state, or outbox cursor.
- Remote relay integration and operator cutover automation.

## Completion Criteria

<!-- lint-ack: decision-coverage - p239 binds one-shot bridge composition, file-backed Matrix fixtures, target-to-room resolution, CLI entrypoint, bearer auth reuse, cursor persistence, and partial parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify JSON files, JSONL output, fake HTTP request paths/headers/bodies, cursor files, CLI parsing, error behavior, and repository Markdown with local doubles only. -->
<!-- lint-ack: boundary-entry-point - agentd-matrix runner/transport tests, agentd-bin CLI/runner tests, and agentctl parity artifact tests cover the listed entry points. -->

Scenario: Matrix room directory resolves direct rooms and trusted targets
  Test:
    Package: agentd-matrix
    Filter: matrix_room_directory_resolves_direct_room_and_trusted_target
  Level: library unit
  Test Double: in-memory room registrations
  Given one trusted group room and one trusted direct-agent room
  When an outbound event already carries `room_id`
  Then the directory returns that room id without consulting target mappings
  And when an outbound event carries only a matching `target`
  Then the directory resolves the trusted room for that target

Scenario: file Matrix transport writes JSONL with resolved rooms
  Test:
    Package: agentd-matrix
    Filter: file_matrix_transport_writes_sent_jsonl_with_resolved_room
  Level: library integration
  Test Double: tempfile filesystem
  Given room and inbound JSON fixture files and a sent-log JSONL path
  When the file Matrix transport sends an outbound event with only `target`
  Then it appends one JSONL record containing `seq`, `roomId`, `target`,
  `messageId`, `source`, `body`, and the raw payload

Scenario: file Matrix transport rejects unmapped outbound targets
  Test:
    Package: agentd-matrix
    Filter: file_matrix_transport_rejects_unmapped_target_without_writing
  Level: library integration
  Test Double: tempfile filesystem
  Given room registrations that do not contain a trusted mapping for the target
  When the file Matrix transport sends an outbound event for that target
  Then it returns a Matrix transport error containing the target name
  And it does not create or append the sent JSONL file

Scenario: one-shot bridge runner uses HTTP backend, file transport, and cursor state
  Test:
    Package: agentd-matrix
    Filter: matrix_bridge_once_runner_posts_files_polls_outbox_logs_sent_and_saves_cursor
  Level: library integration
  Test Double: local TCP fake agentd HTTP server and tempfile filesystem
  Given a state path, room JSON file, inbound JSON file, sent JSONL path, and a fake agentd HTTP backend
  When the one-shot runner executes
  Then the fake backend receives room and inbound POST requests before the outbox GET
  And the sent JSONL file contains the resolved Matrix room for the backend outbox event
  And the state file persists the confirmed outbox cursor

Scenario: agentd CLI parses one-shot Matrix bridge command and auth
  Test:
    Package: agentd-bin
    Filter: agentd_cli_matrix_bridge_once_accepts_files_api_and_auth
  Level: cli unit
  Test Double: clap parser only
  Given explicit one-shot Matrix bridge file paths and `--api-token`
  When `AgentdCli` parses `agentd matrix-bridge-once`
  Then the parsed command carries the API URL and all fixture/state paths
  And the global daemon config carries the operator token for the bridge backend

Scenario: agentd-bin one-shot command composes the runner without real Matrix
  Test:
    Package: agentd-bin
    Filter: agentd_bin_matrix_bridge_once_runs_against_fake_agentd
  Level: library integration
  Test Double: local TCP fake agentd HTTP server and tempfile filesystem
  Given an agentd-bin one-shot command configured with file fixtures and a fake HTTP backend
  When `run_matrix_bridge_once` runs
  Then it returns a report for registered rooms, forwarded inbound events, and sent outbound events
  And it writes the sent JSONL record and cursor state without using Matrix SDK, Claude, or the real execute smoke gate

Scenario: parity docs record p239 one-shot bridge progress
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p239_matrix_bridge_once_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p239, `matrix-bridge-once`, file-backed Matrix transport, and target-to-room resolution
  And the Matrix bridge row remains partial
  And the row still names the real Matrix SDK, puppet, media, cutover, rollback, and token gaps
