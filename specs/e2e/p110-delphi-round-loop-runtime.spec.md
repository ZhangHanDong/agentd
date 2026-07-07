spec: task
name: "Delphi review loop advances review rounds"
tags: [e2e, core, workflow, p1, delphi]
---

## Intent

Implement the first executable Delphi N-round loop on top of P108 round-aware
parks and P109 validated `converge_or_*` contracts. The engine can route a
non-final Delphi fan_in result back to the paired fan_out via
`outcome=partial_success`, and the next fan_out invocation creates a new review
run with the next round number.

## Decisions

- `converge_or_<fallback>` fan_in returns `Status::PartialSuccess` before `max_rounds` when the current verdict signature is not yet known to be stable.
- A non-final Delphi fan_in writes `delphi_next_round = current_round + 1` and a deterministic `delphi_previous_verdicts` signature into `Outcome.context_updates`.
- A Delphi fan_out reads `delphi_next_round` from context, defaults to round `1`, stores that round in the new review run, and parks with the same round.
- The round-2 reviewer prompt includes the Delphi round and previous verdict signature so reviewers receive peer-visible context even before findings text exists.
- A Delphi fan_in finishes when the current verdict signature equals `delphi_previous_verdicts` or when `current_round >= max_rounds`; final status is the fallback aggregator result.
- P110's initial convergence mode was `verdict_stable`; P113 extends the same
  loop with `findings_diff<...>` after findings storage lands. Anonymized
  findings digests remain separate work.

## Boundaries

### Allowed Changes

- specs/e2e/p110-delphi-round-loop-runtime.spec.md
- specs/e2e/p109-delphi-contract-and-fallback.spec.md
- specs/core/p2-node-graph-validate.spec.md
- specs/core/p7-handlers-fan-out-fan-in-wait-human.spec.md
- crates/agentd-core/src/graph/validate.rs
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/handler/fan_in.rs
- crates/agentd-core/tests/node_graph.rs
- crates/agentd-core/tests/handlers_park.rs
- crates/agentd-core/tests/engine_execute.rs

### Forbidden

- Do not change review table schema, review ids, or event payload shape.
- Do not implement review bundle files or anonymized findings digests.
- Do not implement per-reviewer stance packs.

## Acceptance Criteria

Scenario: fan_out creates a later Delphi round from context
  Test: fan_out_delphi_run_uses_context_next_round
  Given a Delphi fan_out node and context delphi_next_round=2
  When fan_out runs
  Then it creates a review park with round=2 and reviewer prompts mention Delphi round 2

Scenario: fan_in requests the next Delphi round before max_rounds
  Test: fan_in_converge_or_majority_pass_requests_next_round_before_max
  Given round 1 verdicts with aggregator=converge_or_majority_pass and max_rounds=3
  When fan_in runs without a previous verdict signature
  Then it returns Done with status PartialSuccess and context delphi_next_round=2

Scenario: engine re-parks at round 2 after a non-final Delphi fan_in
  Test: engine_delphi_loop_reparks_second_round_before_max
  Given a validated Delphi workflow whose aggregate node routes outcome=partial_success back to review
  When the first round receives all reviewer verdicts before convergence
  Then the run parks again at the review node with round=2

Scenario: fan_in finishes when Delphi verdicts stabilize
  Test: fan_in_converge_or_majority_pass_finishes_when_verdicts_stabilize
  Given round 2 verdicts matching context delphi_previous_verdicts
  When fan_in runs with aggregator=converge_or_majority_pass
  Then it returns the majority_pass fallback status without requesting another round
