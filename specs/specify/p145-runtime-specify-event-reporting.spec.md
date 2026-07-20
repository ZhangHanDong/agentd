spec: task
name: "Runtime Specify semantic event reporting"
tags: [p1, specify, events, runtime]
---

<!-- lint-ack: output-mode-coverage - this slice reports runtime semantic events through an in-process trait, not CLI stdout/file output modes. -->
<!-- lint-ack: bdd-rule-grouping - the scenarios cover one focused runtime reporting hook. -->

## Intent

P144 added a tested helper that maps durable agentd events into Specify semantic
events and reports them through `SpecifyClient`. Runtime still never calls that
helper. Add a bounded `ProductionRunHost` hook so each newly appended durable
state-change event can also be reported to Specify, while standalone operation
remains no-network by default and optional reporting cannot change run
advancement, durable event storage, or live event delivery.

## Decisions

- Add an `agentd-specify` dependency to `agentd-bin`, without adding real
  HTTP/WS transport or network dependencies.
- `ProductionRunHost` owns an injectable `SpecifyClient` and defaults it to
  `OfflineSpecify`.
- Add a test/future-config builder that swaps the default Specify client without
  changing daemon startup configuration in this slice.
- Call `report_agentd_event` only after `event_repo::append` succeeds for a new
  durable event.
- Broadcast the existing live event before optional Specify reporting, so live
  observers are not blocked by reporting errors.
- Treat Specify reporting as best-effort in runtime: swallow reporting errors
  after durable append/live broadcast so optional reporting never fails the run.
- Use the local `run_id` string as the temporary `workflow_id` until real Specify
  dispatch provides a canonical external workflow identifier.
- Preserve existing deduplication and ignored-progress semantics: suppressed
  duplicate re-parks and `RunProgress::Ignored` do not report because no durable
  event is appended.
- Update the P144 source-inspection test that treated "no runtime wiring" as the
  current invariant; after P145, the current invariant is narrower:
  `agentd-specify` must not depend back on runtime or network crates.

## Boundaries

### Allowed Changes

- crates/agentd-bin/Cargo.toml
- crates/agentd-bin/src/host.rs
- crates/agentd-specify/tests/events.rs
- specs/specify/p145-runtime-specify-event-reporting.spec.md

### Forbidden

- Do not add real Specify HTTP/WS transport, auth, endpoint config, environment
  variables, or background replay tasks.
- Do not add network dependencies such as `reqwest`, `tokio-tungstenite`, or
  `url` to `agentd-bin`.
- Do not change durable event storage, event repository schema, SSE/live event
  payload shape, P143 semantic mapping, or `EventRecord`.
- Do not wire `agentd-core`, `agentd-surface`, workflow DOT files, or dashboard
  code to `agentd-specify`.
- Do not fail `ProductionRunHost::emit` because Specify reporting fails after a
  durable event has already been appended.

## Out of Scope

- Real Specify API endpoint contracts.
- Runtime configuration that chooses an online Specify client.
- Mapping real external Specify workflow ids to agentd run ids.
- Backfilling or replaying historical durable events.
- Additional semantic event kinds beyond the P143/P144 mapping helper.

## Completion Criteria

Scenario: appended runtime events are reported through injected SpecifyClient
  Test:
    Package: agentd-bin
    Filter: production_runhost_reports_appended_events_to_specify_client
  Level: runtime hook contract
  Test Double: recording SpecifyClient
  Given a `ProductionRunHost` with a recording Specify client
  And a recorded run id "specify-report"
  And a runtime park node named "implement"
  When the host emits `RunProgress::Parked` for node "implement"
  Then the durable event store contains one `run_parked` event
  And the recording client captures one semantic event
  And the semantic event workflow id is "specify-report"
  And the semantic event kind and payload match the P143 mapping

Scenario: dedup-suppressed re-parks are not reported
  Test:
    Package: agentd-bin
    Filter: production_runhost_does_not_report_deduped_reparks
  Level: runtime hook contract
  Test Double: recording SpecifyClient
  Given a `ProductionRunHost` with a recording Specify client
  And an existing durable `run_parked` event for node "implement"
  When the host emits the same `RunProgress::Parked` again
  Then no second durable event is appended
  And no second semantic event is reported

Scenario: Specify reporting errors do not fail runtime emit
  Test:
    Package: agentd-bin
    Filter: production_runhost_ignores_specify_report_errors_after_durable_emit
  Level: runtime resilience contract
  Test Double: failing SpecifyClient
  Given a `ProductionRunHost` with a Specify client that fails `report_event`
  And a live event subscriber
  When the host emits `RunProgress::Finished`
  Then `emit` returns success
  And the durable event store contains the `run_finished` event
  And the live subscriber receives the `run_finished` event

Scenario: default runtime reporting preserves standalone offline behavior
  Test:
    Package: agentd-bin
    Filter: production_runhost_default_specify_reporting_preserves_standalone_mode
  Level: runtime standalone contract
  Test Double: OfflineSpecify
  Given a default `ProductionRunHost`
  When the host emits `RunProgress::Failed`
  Then `emit` returns success
  And the durable event store contains the `run_failed` event
  And no external Specify configuration or network client is required

Scenario: runtime Specify reporting keeps crate boundaries narrow
  Test:
    Package: agentd-bin
    Filter: runtime_specify_reporting_keeps_boundary
  Level: crate boundary contract
  Test Double: source inspection
  Given the `crates/agentd-bin/Cargo.toml` manifest
  And the `crates/agentd-core/Cargo.toml` manifest
  And the `crates/agentd-surface/Cargo.toml` manifest
  When the runtime reporting boundary is inspected
  Then `agentd-bin` depends on `agentd-specify`
  And `agentd-core` and `agentd-surface` do not depend on `agentd-specify`
  And `agentd-bin` does not add `reqwest`, `tokio-tungstenite`, or `url`

Scenario: superseded P144 source inspection keeps the helper boundary current
  Test:
    Package: agentd-specify
    Filter: event_reporting_helper_keeps_runtime_wiring_out
  Level: crate boundary contract
  Test Double: source inspection
  Given the `agentd-specify` manifest and helper source
  When the superseded P144 boundary assertion is inspected after P145
  Then `agentd-specify` still does not depend on `agentd-surface`, `agentd-bin`,
    `reqwest`, or `tokio-tungstenite`
  And the helper source stays decoupled from `EventRecord`
