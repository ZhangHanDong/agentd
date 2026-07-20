spec: task
name: "Specify client foundation with OfflineSpecify seam"
tags: [p1, specify, seam, standalone]
---

## Intent

Track A has landed enough standalone runtime hardening that the next bounded
integration step is the Track B foundation that does not depend on a live Specify
API. Add an `agentd-specify` crate that defines the outbound Specify protocol seam
and a no-network `OfflineSpecify` implementation, so future DOT/tool integration
can depend on a stable local trait while standalone mode remains the default.

## Decisions

- Add a private workspace crate named `agentd-specify`.
- Define an object-safe `SpecifyClient` trait with methods for the boundary Δ7
  operations: pull issue context, push draft, pull frozen spec, report semantic
  event, and report acceptance.
- Define request/response structs for those operations inside the crate. The
  semantic event kind remains an opaque string in this slice; mapping internal
  agentd events to Specify vocabulary is Δ8 and stays out of scope.
- Add `OfflineSpecify` as the default standalone implementation. It must never
  perform network I/O. Required content operations return an explicit offline
  error; reporting operations are no-op successes so optional reporting cannot
  break standalone runs.
- Add a `test-support` recording client that contract tests can use without a
  real Specify server.
- Do not add `reqwest`, WebSocket, auth-token, or real HTTP API code in this
  slice.

## Boundaries

### Allowed Changes

- Cargo.toml
- README.md
- crates/agentd-specify/**
- specs/specify/p142-offline-specify-seam.spec.md

### Forbidden

- Do not modify workflow `.dot` files.
- Do not wire Specify into `agentd-bin`, `agentd-core`, `agentd-surface`, or
  existing runtime behavior.
- Do not add real network transport dependencies or call out to a live Specify
  service.
- Do not implement Specify-web, issue storage ownership, freeze/review UI, or
  canonical lifecycle state.

## Out of Scope

- Real Specify HTTP/WS client implementation.
- Mapping `EventRecord` or engine events to Specify semantic events (Δ8).
- Replacing the standalone `.agentd/run/*` file convention in `draft.dot` or
  `execute.dot`.
- Matrix dispatch wiring for Specify-originated work tokens.

## Completion Criteria

Scenario: agentd-specify is a private workspace crate
  Test:
    Package: agentd-specify
    Filter: specify_crate_is_private_workspace_member
  Level: workspace contract
  Test Double: source inspection
  Given the workspace Cargo.toml and crates/agentd-specify/Cargo.toml
  When the workspace members are inspected
  Then `crates/agentd-specify` is listed as a member
  And the crate is marked `publish = false`

Scenario: OfflineSpecify preserves standalone mode without network dependencies
  Test:
    Package: agentd-specify
    Filter: offline_specify_preserves_standalone_mode
  Level: client seam contract
  Test Double: OfflineSpecify
  Given an `OfflineSpecify` client
  When required content operations are called
  Then they return the `offline` error code
  And reporting operations return success without recording network transport
  And the crate manifest does not depend on `reqwest`, `tokio-tungstenite`, or `url`

Scenario: the SpecifyClient trait is object-safe
  Test:
    Package: agentd-specify
    Filter: specify_client_trait_is_object_safe
  Level: API contract
  Test Double: OfflineSpecify behind dyn SpecifyClient
  Given an `Arc<dyn SpecifyClient>`
  When it points at `OfflineSpecify`
  Then the crate compiles and the trait can be used through the dyn object

Scenario: recording client captures the Δ7 protocol operations
  Test:
    Package: agentd-specify
    Filter: recording_specify_client_captures_protocol_operations
  Level: test-support contract
  Test Double: RecordingSpecifyClient
  Given a recording client scripted with issue, draft, and frozen-spec responses
  When all five SpecifyClient operations are called
  Then the calls are recorded in order
  And the scripted values are returned without network I/O

Scenario: README lists agentd-specify as the optional Specify adapter
  Test:
    Package: agentd-specify
    Filter: readme_lists_agentd_specify_optional_adapter
  Level: docs contract
  Test Double: source inspection
  Given README.md
  When the crate layout section is inspected
  Then it lists `agentd-specify`
  And it describes the crate as an optional Specify client or adapter
