spec: task
name: "Runs, node outcomes, and checkpoint repositories"
tags: [store, mvp, p0]
---

## Intent

The core engine-facing persistence: the `runs` lifecycle (`insert_run` /
`update_run_status` / `set_current_node`), `node_outcomes` (`insert_node_outcome`
/ `latest_outcome` / `count_attempts`), and `checkpoints` (`write_checkpoint` /
`load_checkpoint`). Implemented as free functions over the pool returning
`StoreError`; the `ports::Store` trait impl (Task 5) wraps them. Behavior must
match the P0.1 `InMemoryStore` so "implements the same trait" holds.

## Decisions

- `insert_run` writes a MINIMAL row (id + workflow_sha; store fills status='running' + timestamps; project_id/workflow_path stay NULL — the reconciliation proof). Idempotent via `ON CONFLICT(id) DO NOTHING` so a daemon-pre-created rich row survives.
- `update_run_status` / `set_current_node` error `NotFound` on an unknown run (parity with the fake). Terminal statuses stamp `finished_at`.
- `insert_node_outcome` appends at `attempt = MAX(attempt)+1`; the PK `(run_id, node_id, attempt)` makes `count_attempts` a row count and `latest_outcome` the highest-attempt row. `mempal_writes` are not persisted here (they flow to `mempal_outbox`, §3.4) — a reconstructed `Outcome` has empty `mempal_writes`.
- `context_updates`/`artifacts`/`suggested_next_ids` round-trip as JSON TEXT.
- `write_checkpoint` upserts `ON CONFLICT(run_id)`; `load_checkpoint` reconstructs the `Checkpoint`. The run must exist first (FK), which the engine guarantees by calling `insert_run` at execute start.

## Boundaries

### Allowed Changes

- crates/agentd-store/src/{run_repo,outcome_repo,checkpoint_repo,util}.rs and lib.rs
- crates/agentd-store/tests/runs_outcomes.rs

### Forbidden

- Do not error on a re-`insert_run` of the same id (idempotent, matching the fake).
- Do not persist mempal writes in node_outcomes (they belong to the outbox).

## Completion Criteria

Scenario: A minimal run row satisfies the reconciled schema
  Test: insert_run_minimal_row_satisfies_reconciled_schema
  Given a fresh store
  When insert_run is called with only a run id and workflow_sha
  Then the row is written with status running and NULL project_id/workflow_path

Scenario: insert_run is idempotent on the run id
  Test: insert_run_is_idempotent_on_id
  Given a run already inserted
  When insert_run is called again for the same id
  Then it is a no-op and the first workflow_sha is preserved

Scenario: Updating an unknown run errors
  Test: update_run_status_errors_on_unknown_run
  Given a store with no such run
  When update_run_status is called
  Then it returns StoreError::NotFound

Scenario: Run status and current node round-trip
  Test: run_status_and_current_node_round_trip
  Given an inserted run
  When set_current_node and update_run_status(Finished) run
  Then the row reflects the node, finished status, and a finished_at timestamp

Scenario: Node outcome attempts increment and the latest wins
  Test: node_outcome_attempt_increments_and_latest_wins
  Given two outcomes inserted for the same run and node
  When count_attempts and latest_outcome are queried
  Then the count is 2 and latest_outcome is the second (highest-attempt) outcome

Scenario: A node outcome round-trips its context, label, and artifact
  Test: node_outcome_round_trips_context_label_and_artifact
  Given an outcome with a preferred_label, a context update, and an artifact
  When it is inserted and read back via latest_outcome
  Then the label, context value, and artifact survive

Scenario: A checkpoint round-trips and upserts through the store
  Test: checkpoint_round_trips_through_store
  Given a checkpoint written for an existing run
  When it is loaded back, then re-written with a new current node
  Then the first load equals the original and the second reflects the update
