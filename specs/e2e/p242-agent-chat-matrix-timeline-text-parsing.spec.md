spec: task
name: "agent-chat Matrix timeline text parsing"
tags: [agent-chat-replacement, matrix-bridge, matrix-remote, phase-g, p242]
---

## Intent

Move the p241 Matrix SDK adapter past an empty `text_events` placeholder by
normalizing SDK `/sync` timeline room-message events into the existing
`MatrixClientTextMessage` contract. This slice must be fully local and
deterministic: tests use raw SDK/Ruma event values and source inspection, not a
real Matrix homeserver, real Claude, or real execute smoke.

## Decisions

- Add a feature-gated helper named `sdk_timeline_text_messages` behind
  `matrix-sdk-adapter`; it accepts a room id and SDK `SyncTimelineEvent` values
  and returns normalized `MatrixClientTextMessage` values.
- Parse only original `m.room.message` sync timeline events; skip state events,
  redacted message events, non-room-message message-like events, and malformed
  raw events.
- Preserve `event_id`, `room_id`, `sender_mxid`, and the Matrix message body via
  the SDK/Ruma room-message content accessors.
- Preserve explicit Matrix mentions by copying `m.mentions.user_ids` into the
  normalized `mentions` vector as MXID strings.
- Preserve direct reply targets by mapping `Relation::Reply` to
  `reply_to = Some(event_id)`.
- Update `SdkMatrixClient::sync_once` to parse text events from
  `SyncResponse.rooms.join[*].timeline.events` after the SDK sync call, while
  continuing to derive joined and invited room metadata from the SDK room state.
- Keep `matrix_bridge` partial after this slice because agentd still lacks
  puppet account provisioning, room lifecycle parity, Matrix media, bot
  commands, long-running service packaging, cutover, rollback, token
  provisioning/rotation, bridge operations, and dashboard visibility.

## Boundaries

### Allowed Changes

- specs/e2e/p242-agent-chat-matrix-timeline-text-parsing.spec.md
- crates/agentd-matrix/src/lib.rs
- crates/agentd-matrix/tests/sdk_adapter.rs
- crates/agentctl/tests/parity_cli.rs
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- Cargo.lock

### Forbidden

- Do not run real Claude.
- Do not run the real execute smoke gate in this slice.
- Do not mutate the `/Users/zhangalex/Work/Projects/consult/agent-chat`
  checkout.
- Do not connect to a real Matrix homeserver in tests.
- Do not register Matrix bot or puppet accounts.
- Do not upload or download Matrix media.
- Do not add Matrix bot command handling.
- Do not add long-running bridge service packaging, cutover, rollback, token
  provisioning, or token rotation.
- Do not change the p236 agentd HTTP Matrix contract.
- Do not mark the `matrix_bridge` parity row as `covered`.

## Out of Scope

- Real homeserver integration tests, encrypted-room verification, account
  lifecycle, media transfer, bot commands, service installation, operator
  cutover, rollback automation, and dashboard rendering.

## Completion Criteria

<!-- lint-ack: decision-coverage - p242 binds sdk_timeline_text_messages, original room-message filtering, body/mentions/reply extraction, sync response source binding, and parity docs through explicit tests. -->
<!-- lint-ack: observable-decision-coverage - scenarios verify raw event parsing, source binding, default feature gating, and repository Markdown state with local-only tests. -->
<!-- lint-ack: error-path - p242 includes redacted/state/non-room-message/malformed event skips and not-covered parity scenarios. -->

Scenario: SDK timeline parser extracts text body, mentions, and reply metadata
  Test:
    Package: agentd-matrix
    Filter: sdk_timeline_parser_extracts_text_mentions_and_reply_from_raw_sync_events
  Level: library unit
  Test Double: raw Ruma sync timeline event values
  Given raw original `m.room.message` SDK timeline events
  When `sdk_timeline_text_messages` parses them for a Matrix room id
  Then each normalized message preserves `event_id`, `room_id`, `sender_mxid`, and body
  And explicit `m.mentions.user_ids` are returned as mention strings
  And a direct `m.in_reply_to.event_id` relation is returned as `reply_to`

Scenario: SDK timeline parser skips unsupported timeline entries without failing
  Test:
    Package: agentd-matrix
    Filter: sdk_timeline_parser_skips_state_redacted_non_message_and_malformed_events
  Level: library unit
  Test Double: raw Ruma sync timeline event values
  Given state events, redacted room messages, non-room-message message-like events, and malformed raw events
  When `sdk_timeline_text_messages` parses the event list
  Then unsupported entries are omitted
  And valid original room-message entries in the same list are still returned

Scenario: SDK sync path reads text events from the Matrix SDK sync response
  Test:
    Package: agentd-matrix
    Filter: sdk_matrix_client_sync_path_uses_sync_response_timeline_events
  Level: artifact inspection
  Test Double: Rust source text
  Given the `SdkMatrixClient::sync_once` implementation
  When the source is inspected
  Then it binds the SDK sync response to a local value
  And it reads `sync.rooms.join` timeline events
  And it calls `sdk_timeline_text_messages`
  And it no longer hard-codes `text_events: Vec::new()` for the SDK path

Scenario: SDK timeline parser remains feature-gated in default builds
  Test:
    Package: agentd-matrix
    Filter: sdk_timeline_parser_stays_feature_gated_in_default_build
  Level: artifact inspection
  Test Double: Cargo manifest and Rust source text
  Given default `agentd-matrix` builds
  When feature declarations and parser cfg attributes are inspected
  Then `matrix-sdk-adapter` remains disabled by default
  And SDK timeline parser code is gated behind `matrix-sdk-adapter`

Scenario: parity docs record p242 timeline progress without declaring replacement
  Test:
    Package: agentctl
    Filter: parity_capability_map_records_p242_matrix_timeline_progress
  Level: artifact inspection
  Test Double: repository Markdown files
  Given the agent-chat replacement parity map and roadmap
  When the Matrix bridge row and Phase G roadmap are inspected
  Then the Matrix bridge row mentions p242 and SDK timeline text parsing
  And the Matrix bridge row remains partial
  And the row still names puppet accounts, Matrix media, cutover, rollback, and token gaps
