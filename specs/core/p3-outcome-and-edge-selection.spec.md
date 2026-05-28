spec: task
name: "Edge selection priority + condition mini-language"
tags: [core, mvp, p0, workflow]
---

## Intent

After a node produces an Outcome, the engine picks the next edge to follow.
`select_next_edge` implements the attractor-style priority (design §2.4 + D8b)
deterministically, and evaluates edge `condition` attributes through a small
boolean mini-language over the Outcome and the run context.

## Decisions

- `select_next_edge(graph, node_id, outcome, ctx, attempts) -> Option<&EdgeDef>`
- Priority among the node's outgoing edges (source order within a tier):
  1. an edge whose `condition` evaluates true
  2. an edge whose `label` equals `outcome.preferred_label`
  3. a `retry_target=true` edge, only when `outcome.status == Fail` AND the target node's attempts `< retry_policy.max` (D8b/D8c); skipped at the ceiling
  4. an edge whose `to` is in `outcome.suggested_next_ids`
  5. among unconditional edges, the highest `weight` (default 1)
  6. lexical tiebreak on the target id; `None` if nothing matches
- Condition mini-language: atoms `outcome=<x>`, `answer=<x>`, `kv("k")=="v"`, combined with `!`, `&&`, `||`, and parentheses
- `outcome=<x>` is true when `<x>` names the Outcome status (success/fail/retry/partial_success) OR equals `outcome.preferred_label` (so `outcome=goal_gate_unmet`, a synthesized label, matches — D8a)
- `answer=<x>` is true when the context key `answer` equals `<x>`
- `kv("k")=="v"` is true when context key `k` equals `v`
- `retry_policy` max is parsed from the target node's `retry_policy="max=N,..."`; default max 0 (no auto-retry)
- `attempts` is keyed by node id (String); the engine maps from checkpoint counters

## Boundaries

### Allowed Changes

- crates/agentd-core/src/graph/edge_select.rs
- crates/agentd-core/src/graph/mod.rs
- crates/agentd-core/tests/outcome_edge.rs

### Forbidden

- Do not make edge selection nondeterministic (ties resolve lexically).
- Do not take a retry_target edge when the target is at its attempt ceiling.

## Completion Criteria

Scenario: A matching condition wins over everything else
  Test: edge_select_condition_first
  Given a node with a condition edge "outcome=success" and a plain edge
  And an Outcome with status success
  When select_next_edge runs
  Then it returns the condition edge

Scenario: preferred_label is used when no condition matches
  Test: edge_select_falls_back_to_preferred_label
  Given a node with edges labelled "approve" and "reject" and no condition edges
  And an Outcome whose preferred_label is "approve"
  When select_next_edge runs
  Then it returns the "approve" edge

Scenario: retry_target is preferred on Fail when under the attempt ceiling
  Test: edge_select_prefers_retry_target_on_fail_when_under_attempt_ceiling
  Given a Fail Outcome and a retry_target edge whose target has retry_policy max=3
  And the target's attempt count is 1
  When select_next_edge runs
  Then it returns the retry_target edge

Scenario: retry_target is skipped at the attempt ceiling
  Test: edge_select_skips_retry_target_when_attempt_ceiling_reached
  Given a Fail Outcome and a retry_target edge whose target has retry_policy max=2
  And the target's attempt count is already 2
  And a fallback unconditional edge exists
  When select_next_edge runs
  Then it does not return the retry_target edge
  And it returns the fallback edge

Scenario: highest weight wins among unconditional edges
  Test: edge_select_uses_weight_when_no_label
  Given a Success Outcome with no preferred_label and two unconditional edges with weight 1 and weight 5
  When select_next_edge runs
  Then it returns the weight-5 edge

Scenario: lexical tiebreak, and None when nothing matches
  Test: edge_select_lex_tiebreak_and_none_when_no_match
  Given two equal-weight unconditional edges to targets "alpha" and "beta"
  When select_next_edge runs
  Then it returns the edge to "alpha"
  And a node with no outgoing edges returns None
