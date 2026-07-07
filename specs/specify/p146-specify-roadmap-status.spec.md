spec: task
name: "Specify roadmap status reflects P142-P145"
tags: [p1, specify, docs, roadmap]
---

<!-- lint-ack: output-mode-coverage — this spec verifies repository documentation text through source-inspection tests rather than CLI output modes. -->
<!-- lint-ack: error-path — this is a documentation status correction; the negative constraint is covered by source-inspection assertions that real network transport remains absent. -->
<!-- lint-ack: bdd-rule-grouping — the scenarios cover one focused documentation status correction. -->

## Intent

P142 through P145 moved Specify Track B beyond the old "trait plus
OfflineSpecify only" planning state: the repo now has the optional client seam,
semantic event mapping, a reporting helper, and a runtime `ProductionRunHost`
hook. The P1 roadmap and boundary doc still read as if those steps are future
work and as if a thin reqwest transport is the next concrete implementation.
Update the docs so future work starts from the as-built state and does not guess
an external Specify HTTP/WS API that is still outside this repo.

## Decisions

- Update the P1 roadmap status from a pure planning fork to an as-built status
  note that names P142, P143, P144, and P145.
- Record that Track A is already substantially delivered by existing P99-P118
  slices, instead of presenting it as the next unstarted sequence.
- Record that Track B is delivered through the local in-process seam and runtime
  event hook, while real HTTP/WS transport, auth, endpoint config, and canonical
  external workflow ids remain gated on a concrete Specify API contract.
- Update the Specify boundary doc's Δ7/§4.3 wording so it no longer promises a
  "thin reqwest wrapper" as the current P1 task; it should describe the
  optional `agentd-specify` seam as currently implemented and keep real
  transport future-scoped.
- Keep this slice documentation-only: no runtime code, no workflow DOT changes,
  no Cargo dependency changes, and no real HTTP client.

## Boundaries

### Allowed Changes

- docs/plans/2026-06-05-agentd-p1-roadmap.md
- docs/specs/2026-05-29-agentd-specify-boundary.md
- crates/agentd-specify/tests/client.rs
- specs/specify/p146-specify-roadmap-status.spec.md

### Forbidden

- Do not modify runtime crates other than the source-inspection test file.
- Do not add `reqwest`, `tokio-tungstenite`, `url`, auth-token handling, endpoint
  config, background tasks, or a real Specify HTTP/WS client.
- Do not change workflow `.dot` files, durable event storage, semantic mapping,
  `ProductionRunHost`, or the `SpecifyClient` trait.
- Do not invent endpoint paths beyond what the existing boundary doc already
  states.

## Out of Scope

- Implementing real Specify network transport.
- Choosing online/offline runtime configuration.
- Mapping real Specify workflow ids to local run ids.
- Updating unrelated P0/P1 checklist items.

## Completion Criteria

Scenario: P1 roadmap names the current Specify as-built slices
  Test:
    Package: agentd-specify
    Filter: p1_roadmap_records_specify_track_b_as_built_through_p145
  Level: docs status contract
  Test Double: source inspection
  Given the P1 roadmap and specs for P142 through P145
  When the roadmap status is inspected
  Then it names P142, P143, P144, and P145
  And it describes `OfflineSpecify`, semantic event mapping, `report_agentd_event`, and the runtime event hook as already delivered

Scenario: P1 roadmap keeps real Specify transport gated on an external API contract
  Test:
    Package: agentd-specify
    Filter: p1_roadmap_keeps_real_transport_gated_on_external_contract
  Level: docs boundary contract
  Test Double: source inspection
  Given the P1 roadmap
  When the Track B next-work text is inspected
  Then real HTTP/WS transport, auth, endpoint config, and canonical external workflow ids are listed as future work
  And the roadmap says that work waits for a concrete Specify API contract

Scenario: Specify boundary doc reflects the implemented optional seam instead of a promised reqwest wrapper
  Test:
    Package: agentd-specify
    Filter: specify_boundary_doc_reflects_current_optional_seam
  Level: docs boundary contract
  Test Double: source inspection
  Given the Specify boundary doc and the agentd-specify manifest
  When the outbound Specify client boundary is inspected
  Then the doc describes the current `agentd-specify` seam using `OfflineSpecify`, semantic event mapping, and runtime reporting
  And it does not present a "thin reqwest wrapper" as the current implemented state
  And the manifest still does not add `reqwest`, `tokio-tungstenite`, or `url`
