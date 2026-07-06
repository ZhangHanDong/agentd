spec: task
name: "Per-reviewer stance pack prompts"
tags: [e2e, core, p1, review, mempal]
---

## Intent

Complete the remaining P1.4 stance-pack slice after P110's Delphi loop. A
review fan-out must diversify reviewer context by running reviewer-specific
mempal queries and by attaching reviewer-specific prompt profiles, while keeping
the frozen review bundle identity (`context_sha`) shared across reviewers.

## Decisions

- `parallel.fan_out` accepts optional `stance_queries` as a semicolon-separated
  reviewer map: `reviewer=query;reviewer=query`. When present, it must provide
  one non-empty query for every reviewer, and the query strings must be distinct.
- `parallel.fan_out` accepts optional `prompt_profiles` as the same
  reviewer-map format. When present, it must provide one non-empty profile for
  every reviewer.
- For every reviewer with a stance query, `fan_out` calls
  `MempalClient::search(query, "project", "")` before spawning the reviewer.
  Search failures are best-effort and yield an empty stance pack rather than
  aborting the review park.
- Each reviewer spawn prompt preserves the base adversarial/Delphi prompt and
  appends only that reviewer's `prompt_profile`, `stance_pack_query`, and
  stance-pack hits with drawer ids and bodies.
- This slice does not materialize review bundle files, add schema, or export
  `MEMPAL_CONTEXT_PACK`; the existing in-memory `context_sha` remains the shared
  frozen bundle marker for the review run.

## Boundaries

### Allowed Changes

- specs/e2e/p111-reviewer-stance-pack.spec.md
- specs/core/p7-handlers-fan-out-fan-in-wait-human.spec.md
- crates/agentd-core/src/handler/fan_out.rs
- crates/agentd-core/src/test_support/mempal_stub.rs
- crates/agentd-core/tests/handlers_park.rs

### Forbidden

- Do not write review bundle or per-reviewer context-pack files in this slice.
- Do not change review store schema, review ids, park payloads, or Delphi round
  storage.
- Do not weaken reviewer worktree isolation or duplicate-verdict idempotency.

## Acceptance Criteria

Scenario: fan_out gives each reviewer a distinct stance pack
  Test: fan_out_adds_distinct_stance_pack_to_each_reviewer_prompt
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore + MempalStub with query-specific hits
  Given a fan_out node with two reviewers and two stance_queries entries
  When fan_out runs
  Then mempal search is called once with each configured query
  And each reviewer prompt includes only that reviewer's stance-pack hit
  And both prompts keep the same context_sha marker

Scenario: fan_out gives each reviewer its prompt profile
  Test: fan_out_adds_per_reviewer_prompt_profiles
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore + MempalStub
  Given a fan_out node with two reviewers and two prompt_profiles entries
  When fan_out runs
  Then each reviewer prompt includes only that reviewer's prompt_profile

Scenario: incomplete stance query maps are rejected
  Test: fan_out_rejects_incomplete_stance_query_map
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore
  Given a fan_out node whose stance_queries map omits one reviewer
  When fan_out runs
  Then it returns an invariant error naming the missing reviewer
  And no reviewer agent is spawned

Scenario: duplicated stance queries are rejected
  Test: fan_out_rejects_duplicate_stance_queries
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore
  Given a fan_out node whose stance_queries map assigns the same query to two reviewers
  When fan_out runs
  Then it returns an invariant error mentioning distinct stance queries
  And no reviewer agent is spawned
