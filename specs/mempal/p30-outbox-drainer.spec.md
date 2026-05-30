spec: task
name: "Outbox: transactional enqueue (drainer added in Task 3)"
tags: [mempal, store, mvp, p0, outbox]
---

## Intent

`post_action` must not call mempal synchronously (design §3.4). Instead,
`insert_node_outcome` enqueues one `mempal_outbox` row per `Outcome.mempal_write`
in the SAME database transaction as the `node_outcomes` row, so a slow or down
mempal never blocks workflow execution. This task lands the transactional
enqueue and the `outbox_repo` read side; the background drainer is added in
Task 3 (extending this spec).

## Decisions

- `insert_node_outcome` wraps the `node_outcomes` insert and every `mempal_outbox` enqueue in one `sqlx` transaction (`pool.begin()` … `commit()`). A failure anywhere commits NEITHER the outcome nor any outbox row (atomic, §3.4).
- One outbox row per `MempalWrite`: `kind` is the op (`ingest` / `kg_add` / `fact_check`), `payload` is the write's JSON (round-trips back to `MempalWrite`), `enqueued_at` is now, `drained_at` is NULL, `attempts` is 0.
- An outcome with no `mempal_writes` enqueues nothing (the existing engine path is unchanged).
- `outbox_repo::claim_pending(limit)` returns pending rows (`drained_at IS NULL`) FIFO by `enqueued_at`, as `OutboxRow { id, run_id, node_id, kind, payload, attempts }`.
- The shared `run_id` foreign key on both tables means an outbox-only failure is not data-reachable; the transaction's rollback is verified by the failed-insert-leaves-nothing scenario plus review of the single `begin/commit` boundary.
- The background drainer's `drain_once` claims only RETRYABLE rows via `claim_retryable(limit, max_attempts)` (`drained_at IS NULL AND attempts <= max_attempts`, FIFO) and dispatches each `MempalWrite` to the `MempalClient` (Ingest→ingest, KgAdd→kg_add, FactCheck→fact_check); on success it `mark_drained`s the row, on failure it `mark_failed`s (attempts + 1, last_error) and leaves the row pending to retry. When a failure pushes a row's attempts past `max_attempts` (5) it is reported as an operator alert; thereafter `claim_retryable` excludes it, so a permanently-stuck row never starves the claim window or re-runs — it simply stays in the table (still visible to `claim_pending`). `spawn` runs `drain_once` on a loop, backing off exponentially when a pass is idle or erroring.

## Boundaries

### Allowed Changes

- crates/agentd-store/**
- crates/agentd-mempal/**
- specs/mempal/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1) — the MempalWrite and Outcome types already exist there.
- Do not reference, open, or write mempal's on-disk database (MCP-only, §3.1).
- Do not call mempal from the outcome insert — the write only enqueues a row.

## Out of Scope

- The background drainer + its backoff/alert (Task 3, this spec extended); the consistency check (Task 4).

## Completion Criteria

Scenario: a kg_add write enqueues one pending outbox row in the outcome transaction
  Test: test_kg_add_writes_outbox_row_in_same_tx_as_node_outcome
  Given a store with an inserted run and a node outcome carrying one KgAdd mempal write
  When insert_node_outcome runs
  Then the node_outcomes row exists and exactly one pending mempal_outbox row exists whose kind is "kg_add" and whose payload deserializes back to that KgAdd write

Scenario: an ingest write is enqueued, not sent
  Test: test_ingest_via_outbox_does_not_block_workflow
  Given a store with an inserted run and a node outcome carrying one Ingest mempal write
  When insert_node_outcome runs
  Then it returns without contacting mempal and claim_pending returns one row with drained_at unset

Scenario: a failed outcome insert leaves neither row
  Test: enqueue_rolls_back_with_the_outcome_on_failure
  Given a store and a node outcome carrying a mempal write but a run_id that does not exist
  When insert_node_outcome runs
  Then it returns an error and neither a node_outcomes row nor a mempal_outbox row exists for that run

Scenario: an outcome without mempal writes enqueues nothing
  Test: outcome_without_writes_enqueues_nothing
  Given a store with an inserted run and a node outcome with no mempal writes
  When insert_node_outcome runs
  Then the node_outcomes row exists and claim_pending returns no rows

Scenario: the drainer dispatches pending rows and marks them drained
  Test: drainer_drains_pending_rows_and_marks_drained
  Given a store with two enqueued writes (a kg_add and an ingest) and a recording mempal client
  When drain_once runs
  Then the report shows two drained, claim_pending then returns nothing, and the client recorded both writes

Scenario: the drainer retries a failing row until the attempt bound then alerts
  Test: test_drainer_retries_with_backoff_until_attempts_exceeded
  Given a store with one enqueued write and a mempal client that always errors
  When drain_once runs repeatedly
  Then the row's attempts climb to 6, it stays pending (never drained), and a pass reports it as an alert

Scenario: a down mempal leaves the rows pending without erroring the drainer
  Test: drainer_tolerates_mempal_down
  Given a store with one enqueued write and a mempal client that always errors
  When drain_once runs once
  Then it returns Ok, the row is still pending with attempts incremented, and no run or outcome state changed

Scenario: an exhausted row is excluded from claim_retryable so it cannot starve newer writes
  Test: drainer_does_not_reclaim_exhausted_rows
  Given a store with one write driven past the attempt bound by an always-erroring client
  When claim_retryable and claim_pending are queried
  Then claim_retryable returns nothing while claim_pending still lists the stuck row

Scenario: the drainer dispatches a fact_check write
  Test: drainer_drains_a_fact_check_write
  Given a store with one enqueued FactCheck write and a recording mempal client
  When drain_once runs
  Then the report shows one drained and claim_pending then returns nothing

Scenario: an outcome with several writes enqueues one row per write in order
  Test: enqueue_writes_one_row_per_write_in_order
  Given a store with an inserted run and a node outcome carrying a KgAdd then an Ingest write
  When insert_node_outcome runs
  Then claim_pending returns two rows whose kinds are "kg_add" then "ingest" in that order
