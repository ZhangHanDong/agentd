spec: task
name: "Specify mapped event reporting helper"
tags: [p1, specify, events, seam]
---

<!-- lint-ack: output-mode-coverage — this spec reports semantic event payloads through an in-process trait, not CLI stdout/file output modes. -->
<!-- lint-ack: bdd-rule-grouping — the scenarios are one focused helper contract. -->

## Intent

P143 added the pure mapping from durable agentd run events to Specify semantic
events. The next bounded step is to connect that mapper to the existing
`SpecifyClient::report_event` seam inside `agentd-specify`, so future runtime
wiring has one tested helper while standalone mode still defaults to no-network
behavior.

## Decisions

- Add an async helper in `agentd-specify` that maps an `AgentdEventRef` and, only
  when the event is mapped, calls `SpecifyClient::report_event`.
- Return `Ok(true)` when a mapped event was handed to the client successfully.
- Return `Ok(false)` when the local event kind is unknown, and do not call the
  client in that case.
- Preserve P143 decode behavior: malformed JSON for a known mapped event returns
  `SpecifyError` code `decode` before any client call.
- Propagate `SpecifyClient::report_event` errors unchanged after successful
  mapping.
- Preserve standalone mode: `OfflineSpecify` remains a no-network client whose
  reporting methods are no-op successes.
- Keep runtime integration out of this slice; no `agentd-bin`, `agentd-core`, or
  `agentd-surface` hooks are added.

## Boundaries

### Allowed Changes

- crates/agentd-specify/src/events.rs
- crates/agentd-specify/src/lib.rs
- crates/agentd-specify/tests/events.rs
- specs/specify/p144-specify-event-reporting.spec.md

### Forbidden

- Do not wire the helper into `agentd-bin`, `agentd-core`, `agentd-surface`,
  workflow DOT files, daemon startup, or the SSE/dashboard path.
- Do not add real Specify HTTP/WS transport, auth, background tasks, or network
  dependencies.
- Do not change the P143 mapping table or the `SemanticEvent` JSON payload shape.
- Do not change `OfflineSpecify` content operations from explicit offline errors
  or reporting operations from no-op successes.

## Out of Scope

- Real Specify API endpoint contracts.
- Runtime configuration that chooses online vs offline Specify clients.
- Backfilling or replaying historical events.
- Adding richer semantic events such as `task.claimed` or `criterion.passed`.

## Completion Criteria

Scenario: mapped local events are reported through SpecifyClient
  Test:
    Package: agentd-specify
    Filter: mapped_agentd_event_is_reported_through_specify_client
  Level: reporting helper contract
  Test Double: RecordingSpecifyClient
  Given workflow "wf1"
  And a local event for run "r1" with seq 7, kind "run_parked", and payload `{"node":"review","round":1}`
  When the reporting helper is called
  Then it returns `Ok(true)`
  And the recording client captures exactly one `report_event` call
  And the reported event kind is "agent.blocked" with the mapped P143 payload

Scenario: unknown local events are ignored without client calls
  Test:
    Package: agentd-specify
    Filter: unknown_agentd_event_is_not_reported
  Level: reporting helper contract
  Test Double: RecordingSpecifyClient
  Given a local event with kind "state_resync" and malformed payload `{not-json`
  When the reporting helper is called
  Then it returns `Ok(false)`
  And the recording client captures no calls

Scenario: malformed mapped payloads fail before reporting
  Test:
    Package: agentd-specify
    Filter: invalid_event_payload_is_not_reported
  Level: reporting helper contract
  Test Double: RecordingSpecifyClient
  Given a local event with kind "run_parked" and malformed payload `{not-json`
  When the reporting helper is called
  Then it returns an error with code `decode`
  And the recording client captures no calls

Scenario: client report errors propagate after mapping
  Test:
    Package: agentd-specify
    Filter: client_report_event_error_propagates_after_mapping
  Level: reporting helper contract
  Test Double: failing SpecifyClient
  Given a local event with kind "run_finished" and payload `{}`
  And a SpecifyClient whose `report_event` returns a `transport` error
  When the reporting helper is called
  Then it returns the same `transport` error

Scenario: offline reporting remains a standalone no-op success
  Test:
    Package: agentd-specify
    Filter: offline_event_reporting_preserves_standalone_noop
  Level: standalone seam contract
  Test Double: OfflineSpecify
  Given an `OfflineSpecify` client
  And a local event with kind "run_failed" and payload `{"reason":"boom"}`
  When the reporting helper is called
  Then it returns `Ok(true)`
  And no network dependency is required

Scenario: reporting helper keeps runtime and network wiring out
  Test:
    Package: agentd-specify
    Filter: event_reporting_helper_keeps_runtime_wiring_out
  Level: crate boundary contract
  Test Double: source inspection
  Given the workspace and `agentd-specify` manifests
  When the reporting helper boundary is inspected
  Then `agentd-specify` does not depend on `agentd-surface`, `agentd-bin`, `reqwest`, or `tokio-tungstenite`
  And `agentd-bin` does not call `report_agentd_event`
