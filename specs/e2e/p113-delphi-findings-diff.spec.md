spec: task
name: "Delphi findings storage + findings_diff convergence"
tags: [e2e, core, store, surface, p1, delphi]
---

## Intent

Complete the remaining P1.4 Delphi convergence slice that P110 left rejected:
reviewer findings must flow from `submit_review` through the engine event into
the store, and a Delphi fan_out may declare `convergence=findings_diff<N>`.
This gives the N-round review loop a textual convergence signal in addition to
the existing verdict-stability signal.

## Decisions

- `ReviewVerdict` gains an opaque `findings: String`; the store persists and
  returns it with the first accepted verdict for each `(review_run_id,
  reviewer_id)`.
- `submit_review.findings` remains a JSON array at the MCP surface and is
  serialized into a deterministic JSON string before building
  `EngineEvent::ReviewVerdictSubmitted`.
- `convergence=findings_diff<N>` is accepted only for Delphi fan_out nodes when
  `N` parses as a finite `0.0..=1.0` threshold.
- `fan_in` treats Delphi as converged when verdicts are stable, or, for
  `findings_diff<N>`, when the normalized textual diff between the previous and
  current findings signatures is `<= N`.
- A non-final, non-converged Delphi fan_in writes both
  `delphi_previous_verdicts` and `delphi_previous_findings` into context so the
  next round can compare against the authoritative previous round.
- The diff algorithm is deterministic and dependency-free: normalize whitespace,
  sort findings by reviewer id, compute Levenshtein distance over chars, and
  divide by the longer normalized signature length.

## Boundaries

### Allowed Changes

- specs/e2e/p113-delphi-findings-diff.spec.md
- specs/e2e/p110-delphi-round-loop-runtime.spec.md
- specs/core/p2-node-graph-validate.spec.md
- specs/core/p7-handlers-fan-out-fan-in-wait-human.spec.md
- specs/store/p5-review-task-and-store-trait.spec.md
- specs/surface/p74-mcp-tool-error-codes.spec.md
- crates/agentd-core/**
- crates/agentd-store/**
- crates/agentd-surface/**
- crates/agentd-bin/tests/**
- crates/agentctl/tests/**

### Forbidden

- Do not add a new migration for findings; `review_verdicts.findings` already
  exists in the base schema.
- Do not change the public verdict vocabulary (`pass`, `concern`, `blocker`).
- Do not make duplicate reviewer verdicts overwrite the first stored findings.
- Do not add a new dependency for text diffing.

## Out of Scope

- Semantic embeddings or LLM-based findings comparison.
- Anonymized findings digests.
- Review bundle files or `MEMPAL_CONTEXT_PACK`.

## Acceptance Criteria

Scenario: findings_diff convergence validates for Delphi fan_out
  Test: node_graph_accepts_delphi_findings_diff_convergence
  Given a Delphi fan_out with max_rounds=3 and convergence="findings_diff<0.1>"
  When NodeGraph::from_ast runs
  Then it returns Ok

Scenario: malformed findings_diff convergence is rejected
  Test: node_graph_rejects_malformed_delphi_findings_diff_convergence
  Given a Delphi fan_out with convergence="findings_diff<sideways>"
  When NodeGraph::from_ast runs
  Then it returns an error mentioning findings_diff

Scenario: submit_review forwards JSON-array findings into the engine event
  Test: submit_review_forwards_findings_to_engine_event
  Given a submit_review input whose findings field is a JSON array with one structured finding
  When submit_review delivers the review verdict
  Then the delivered ReviewVerdictSubmitted event includes the serialized findings string

Scenario: review verdict findings persist and first writer wins
  Test: review_verdict_findings_round_trip_first_wins
  Given a review run and two verdict submissions from the same reviewer with different findings
  When the verdicts are inserted and listed
  Then only one verdict is counted and its findings are the first submitted findings

Scenario: findings_diff requests another round when findings changed above threshold
  Test: fan_in_findings_diff_requests_next_round_when_findings_changed_above_threshold
  Given a non-final Delphi round with previous findings and current findings whose normalized diff is greater than 0.1
  When fan_in runs
  Then it returns PartialSuccess with delphi_next_round and updated delphi_previous_findings

Scenario: findings_diff converges when findings change below threshold
  Test: fan_in_findings_diff_finishes_when_findings_change_below_threshold
  Given a non-final Delphi round with changed verdicts but current findings whose normalized diff is less than or equal to 0.5
  When fan_in runs
  Then it finishes with the fallback aggregator result and does not request another round

Scenario: findings_diff falls back at max_rounds
  Test: fan_in_findings_diff_uses_fallback_at_max_rounds
  Given a final Delphi round whose findings still differ above threshold
  When fan_in runs
  Then it finishes with the fallback aggregator result instead of requesting another round
