spec: task
name: "Review/task repos + the Store trait impl"
tags: [store, mvp, p0]
---

## Intent

The review (`fan_out`/`fan_in`) and task (`codergen`) park repositories, plus
the `impl agentd_core::ports::Store for SqliteStore` that wires every repo into
the one engine-facing trait. The proof of correctness is behavioral parity with
the P0.1 `InMemoryStore`: the same replay/idempotency invariants, and the real
engine driving the full canonical park/resume flow against the real database.

## Decisions

- `insert_review_verdict` is idempotent per reviewer (`ON CONFLICT(review_run_id, reviewer_id) DO NOTHING`), so `count_verdicts` counts DISTINCT reviewers and a replayed verdict cannot reach quorum early.
- `lookup_park_by_review_run` returns `Some` only while `count(verdicts) < expected`; `review_expected` reads the stored count; `list_verdicts` reconstructs `ReviewVerdict`s.
- `insert_task_run` generates the id; `complete_task_run` sets `finished_at`; `lookup_park_by_task_run` returns `Some` only while `finished_at IS NULL` (so a replayed agent outcome no-ops).
- The trait impl delegates each method to its repo; repos return `StoreError` and `?` converts to the trait's `CoreError`.

## Boundaries

### Allowed Changes

- crates/agentd-store/src/{review_repo,task_repo,store_impl}.rs and lib.rs
- crates/agentd-store/tests/store_trait.rs

### Forbidden

- Do not let a duplicate reviewer verdict double-count, or a completed task run keep parking (replay-safety parity with the fake).

## Completion Criteria

Scenario: Review verdicts dedupe per reviewer and the park opens/closes on quorum
  Test: review_verdict_dedup_and_open_closed_parity
  Given a review run expecting three reviewers
  When verdicts arrive (including a duplicate reviewer) and quorum is reached
  Then the duplicate does not count, the park is open below quorum and closed at quorum

Scenario: Completing a task run closes its park
  Test: task_run_complete_closes_park_parity
  Given an open task run that resolves via lookup_park_by_task_run
  When complete_task_run is called
  Then the lookup returns None afterward

Scenario: The engine runs the full canonical flow against the real store
  Test: engine_runs_canonical_flow_against_sqlite_store
  Given the engine wired with SqliteStore and the in-memory fakes for the other ports
  When it executes the canonical wait.human -> codergen -> fan_out -> fan_in(goal_gate) -> terminal flow via deliver_event
  Then the final progress is Finished and the run row is persisted finished
