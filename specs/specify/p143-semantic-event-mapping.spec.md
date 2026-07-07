spec: task
name: "Specify semantic-event mapping"
tags: [p1, specify, events, seam]
---

<!-- lint-ack: output-mode-coverage — this spec maps event payloads, not CLI stdout/file output modes. -->
<!-- lint-ack: bdd-rule-grouping — the six scenarios are one focused mapping contract. -->

## Intent

P142 introduced the optional `agentd-specify` seam and intentionally kept
`SemanticEvent.kind` opaque. Boundary Δ8 now needs the first stable mapping from
agentd's durable local run events to Specify semantic-event vocabulary, while
still preserving standalone mode and avoiding any runtime or network wiring.

## Decisions

- Add a pure mapping layer inside `agentd-specify` that converts the current
  durable agentd event kinds into `SemanticEvent` values:
  - `run_parked` → `agent.blocked`
  - `run_finished` → `workflow.finished`
  - `run_failed` → `workflow.failed`
- Preserve the local event reference in the semantic payload with `run_id`,
  `seq`, `agentd_event_kind`, and the decoded original compact JSON `payload`.
- Ignore unknown local event kinds with `Ok(None)`, because dashboard-only and
  future local events should not fail optional Specify reporting.
- Return a `decode` error for malformed JSON payloads on known mapped events.
- Keep richer semantic events such as `task.claimed` and `criterion.passed` out
  of this slice until the runtime emits those facts explicitly.
- Do not depend on `agentd-surface::EventRecord`; the mapper accepts a small
  crate-local event reference so this crate remains an optional Specify
  reporting seam.

## Boundaries

### Allowed Changes

- crates/agentd-specify/src/events.rs
- crates/agentd-specify/src/lib.rs
- crates/agentd-specify/tests/events.rs
- specs/specify/p143-semantic-event-mapping.spec.md

### Forbidden

- Do not wire Specify reporting into `agentd-bin`, `agentd-core`,
  `agentd-surface`, workflow DOT files, or runtime execution paths.
- Do not change the durable `EventRecord` shape or existing `run_parked`,
  `run_finished`, `run_failed`, or SSE payload semantics.
- Do not add real Specify HTTP/WS transport, auth, background tasks, or network
  dependencies.
- Do not claim support for `task.claimed` or `criterion.passed` until runtime
  facts exist for those events.

## Out of Scope

- Sending mapped events through `SpecifyClient::report_event`.
- Real Specify API implementation or endpoint contracts.
- Runtime hooks that decide when to report events.
- Replacing the local dashboard/SSE event vocabulary.

## Completion Criteria

Scenario: parked runs map to Specify agent.blocked events
  Test:
    Package: agentd-specify
    Filter: run_parked_maps_to_agent_blocked_with_node_and_round
  Level: semantic mapping contract
  Test Double: pure mapper
  Given workflow "wf1"
  And a local event for run "r1" with seq 7, kind "run_parked", and payload `{"node":"review","round":1}`
  When it is mapped for workflow "wf1"
  Then the mapper returns one `SemanticEvent`
  And the event kind is "agent.blocked"
  And the payload preserves run_id "r1", seq 7, original kind "run_parked", node "review", and round 1

Scenario: finished runs map to Specify workflow.finished events
  Test:
    Package: agentd-specify
    Filter: run_finished_maps_to_workflow_finished
  Level: semantic mapping contract
  Test Double: pure mapper
  Given workflow "wf1"
  And a local event for run "r1" with seq 8, kind "run_finished", and payload `{}`
  When it is mapped for workflow "wf1"
  Then the mapper returns one `SemanticEvent`
  And the event kind is "workflow.finished"
  And the payload preserves the original empty JSON payload

Scenario: failed runs map to Specify workflow.failed events
  Test:
    Package: agentd-specify
    Filter: run_failed_maps_to_workflow_failed_with_reason
  Level: semantic mapping contract
  Test Double: pure mapper
  Given workflow "wf1"
  And a local event for run "r1" with seq 9, kind "run_failed", and payload `{"reason":"boom"}`
  When it is mapped for workflow "wf1"
  Then the mapper returns one `SemanticEvent`
  And the event kind is "workflow.failed"
  And the payload preserves reason "boom"

Scenario: unmapped local events are ignored
  Test:
    Package: agentd-specify
    Filter: unknown_agentd_event_kind_is_ignored
  Level: semantic mapping contract
  Test Double: pure mapper
  Given a local event with kind "state_resync"
  When it is mapped
  Then the mapper returns `Ok(None)`
  And it does not require the payload to be valid JSON

Scenario: mapped events require valid JSON payloads
  Test:
    Package: agentd-specify
    Filter: invalid_event_payload_is_decode_error
  Level: semantic mapping contract
  Test Double: pure mapper
  Given a local event with kind "run_parked" and payload `{not-json`
  When it is mapped
  Then the mapper returns an error
  And the error code is "decode"

Scenario: semantic mapping keeps runtime and network wiring out
  Test:
    Package: agentd-specify
    Filter: semantic_mapping_keeps_runtime_wiring_out_of_specify_crate
  Level: crate boundary contract
  Test Double: source inspection
  Given the workspace and agentd-specify manifests
  When the mapping crate boundary is inspected
  Then `agentd-specify` does not depend on `agentd-surface`, `agentd-bin`, `reqwest`, or `tokio-tungstenite`
  And the mapper source does not import `EventRecord`
