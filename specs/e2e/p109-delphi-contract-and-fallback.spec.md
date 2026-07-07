spec: task
name: "Delphi workflow contract and converge fallback"
tags: [e2e, core, workflow, p1, delphi]
---

## Intent

Move P1.4 Delphi review from a reserved syntax to a validated, executable
workflow contract. This slice does not implement the N-round re-entry loop or
per-reviewer stance packs yet; it makes the graph layer accept only well-formed
Delphi fan_out/fan_in pairs and makes `fan_in` evaluate `converge_or_<fallback>`
with the same fallback aggregators that one-shot review already supports.

## Decisions

- A `parallel.fan_out` node with `visibility=delphi` must set `max_rounds` to an integer `>= 2`.
- A Delphi fan_out must have exactly one reachable `parallel.fan_in` partner in this slice; multi-pair disambiguation remains out of scope until `pair_with` is implemented.
- The paired fan_in must use `aggregator=converge_or_<fallback>`.
- Valid fallback aggregators are exactly `any_fail`, `majority_pass`, `unanimous_pass`, and `first_blocker`.
- A non-Delphi fan_out may keep `max_rounds` unset or `1`; `max_rounds >= 2` requires `visibility=delphi` and a converge aggregator.
- `FanInHandler` strips the `converge_or_` prefix and applies the fallback aggregator to the final available verdict set. Multi-round convergence and previous-round comparison are intentionally deferred to the next P1.4 slice.
- Existing `visibility=blind` workflows and one-shot aggregators remain valid.

## Boundaries

### Allowed Changes

- specs/e2e/p109-delphi-contract-and-fallback.spec.md
- specs/core/p2-node-graph-validate.spec.md
- specs/core/p7-handlers-fan-out-fan-in-wait-human.spec.md
- crates/agentd-core/src/graph/validate.rs
- crates/agentd-core/src/handler/fan_in.rs
- crates/agentd-core/tests/node_graph.rs
- crates/agentd-core/tests/handlers_park.rs

### Forbidden

- Do not treat this P109 contract as proof of Delphi N-round re-entry, anonymized peer finding digests, or per-reviewer stance packs; P110+ owns those behaviors.
- Do not change the review store schema or park payload shape; P108 already supplied the round discriminator.
- Do not relax graph validation into accepting unknown aggregators.

## Acceptance Criteria

Scenario: A well-formed Delphi fan_out/fan_in pair validates
  Test: node_graph_accepts_delphi_visibility_with_converge_aggregator
  Given a graph with visibility=delphi, max_rounds=3, and aggregator=converge_or_majority_pass
  When NodeGraph::from_ast runs
  Then validation succeeds

Scenario: Delphi visibility without max_rounds is rejected
  Test: node_graph_rejects_delphi_visibility_without_max_rounds
  Given a graph whose fan_out declares visibility=delphi without max_rounds
  When NodeGraph::from_ast runs
  Then it returns an error mentioning max_rounds

Scenario: Delphi visibility with a one-shot aggregator is rejected
  Test: node_graph_rejects_delphi_visibility_with_non_converge_aggregator
  Given a graph whose Delphi fan_out reaches fan_in aggregator=majority_pass
  When NodeGraph::from_ast runs
  Then it returns an error mentioning converge_or

Scenario: Unknown converge fallback is rejected
  Test: node_graph_rejects_unknown_converge_fallback
  Given a graph whose fan_in declares aggregator=converge_or_sideways
  When NodeGraph::from_ast runs
  Then it returns an error mentioning sideways

Scenario: max_rounds greater than one requires Delphi visibility
  Test: node_graph_rejects_non_delphi_max_rounds_above_one
  Given a graph whose fan_out declares visibility=blind and max_rounds=3
  When NodeGraph::from_ast runs
  Then it returns an error mentioning visibility=delphi

Scenario: converge_or_majority_pass uses the majority_pass fallback
  Test: fan_in_converge_or_majority_pass_uses_majority_fallback
  Given a final review round with two Pass verdicts and one Fail verdict
  When fan_in runs with aggregator=converge_or_majority_pass
  Then it returns Done with status Success
