spec: task
name: "Human-waits repository"
tags: [store, mvp, p0]
---

## Intent

`open_human_wait` / `answer_human_wait` / `lookup_park_by_wait_id` backing the
`wait.human` handler. The park-open invariant is `answered_at IS NULL` —
mirroring the in-memory fake's `answer.is_none()` — so a resolved or replayed
wait resolves to `None` and the engine no-ops it.

## Decisions

- `open_human_wait` generates the `wait_id` (ulid) and returns it; only id/run/node/prompt are set (interviewer/options are P0.6, nullable).
- `answer_human_wait` updates `WHERE answered_at IS NULL`; zero rows affected → `Conflict` (unknown or already answered), matching the fake's answer-once error.
- `lookup_park_by_wait_id` returns `Some((run, node))` only while `answered_at IS NULL`.

## Boundaries

### Allowed Changes

- crates/agentd-store/src/human_wait_repo.rs and lib.rs
- crates/agentd-store/tests/store_trait.rs

### Forbidden

- Do not let a second answer succeed or a resolved wait keep parking (replay-safety).

## Completion Criteria

Scenario: A human wait answers once, then parks no more and conflicts on re-answer
  Test: human_wait_answer_once_then_conflict_parity
  Given an open human wait that resolves via lookup_park_by_wait_id
  When it is answered, then looked up again, then answered a second time
  Then the post-answer lookup is None and the second answer errors
