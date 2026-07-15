spec: task
name: "Outcome and checkpoint commit atomically"
tags: [e2e, p1, store, checkpoint, recovery]
---

## Intent

The P0.9 deployment checklist still lists an advisor gap where a crash after a
node outcome insert but before the following checkpoint write can leave a
duplicate-able node. This slice closes that specific engine/store seam by
committing each completed-node outcome and its next checkpoint through one store
operation, so continuation and retry checkpoints cannot lag behind their outcome
rows.

## Decisions

- Add a `Store` port operation that commits one `node_outcomes` row and the
  resulting `Checkpoint` atomically.
- `SqliteStore` implements that operation in one SQL transaction that also keeps
  existing `mempal_outbox` enqueue behavior in the same transaction as the
  outcome row.
- `InMemoryStore` implements the same operation under its single mutex so tests
  preserve store-port parity.
- `Engine::process_done` uses the atomic operation on every `Done` path that
  writes a checkpoint after the outcome: retry-with-budget and advance-to-next.
- Goal-gate evaluation overlays the current in-memory outcome while deciding a
  terminal transition, so the engine no longer needs to insert the current
  outcome before computing the checkpoint.
- Keep existing separate APIs for tests and callers that only need an outcome or
  only need a checkpoint; this slice does not remove store methods or change DB
  schema.
- Update the deployment checklist so it no longer lists the outcome/checkpoint
  split as an open known gap after P138.

## Boundaries

### Allowed Changes

- specs/e2e/p138-outcome-checkpoint-atomicity.spec.md
- docs/p0.9-deployment-checklist.md
- crates/agentd-core/src/ports/store.rs
- crates/agentd-core/src/engine/execute.rs
- crates/agentd-core/src/test_support/in_memory_store.rs
- crates/agentd-core/tests/engine_execute.rs
- crates/agentd-store/src/checkpoint_repo.rs
- crates/agentd-store/src/outcome_repo.rs
- crates/agentd-store/src/store_impl.rs
- crates/agentd-store/tests/runs_outcomes.rs
- crates/agentd-bin/tests/deployment_checklist.rs

### Forbidden

- Do not add, remove, or rename database columns or tables.
- Do not change `node_outcomes`, `checkpoints`, or `mempal_outbox` wire/storage
  JSON shapes.
- Do not change HTTP, MCP, `submit_outcome`, or `assign_task` JSON shapes.
- Do not change workflow `.dot` files.

## Out of Scope

- Making terminal `Finished`/`Failed` status updates part of this same
  transaction.
- SSE field sanitization or event emission transactionality.
- Real SIGKILL smoke execution.
- Changing retry policy semantics or adding new retry graph attributes.

## Completion Criteria

Scenario: sqlite rolls back outcome when checkpoint fails
  Test:
    Package: agentd-store
    Filter: outcome_checkpoint_commit_rolls_back_outcome_when_checkpoint_fails
  Level: store contract
  Test Double: real SqliteStore on tempfile
  Given an existing run and an atomic outcome-plus-checkpoint commit whose checkpoint references an unknown run
  When the checkpoint write fails inside the transaction
  Then no `node_outcomes` row remains for the existing run
  And no `mempal_outbox` row remains for that outcome

Scenario: engine uses atomic commit for completed-node checkpoint
  Test:
    Package: agentd-core
    Filter: engine_commits_done_outcome_and_checkpoint_together
  Level: engine contract
  Test Double: store spy over in-memory state
  Given a graph where a regular node completes and advances to another node
  When the engine processes that `Done` node
  Then it calls the store's atomic outcome-plus-checkpoint operation
  And it does not call separate outcome/checkpoint methods for that completed-node checkpoint

Scenario: goal gate still sees current outcome before commit
  Test:
    Package: agentd-core
    Filter: engine_goal_gate_uses_pending_outcome_before_atomic_commit
  Level: engine contract
  Test Double: in-memory store with fake runner
  Given a goal-gate node routes directly to a terminal
  When the node succeeds
  Then the run still reaches `Finished`
  And the latest stored goal-gate outcome is success

Scenario: deployment checklist marks checkpoint atomicity resolved
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_marks_p138_checkpoint_atomicity_resolved
  Level: docs regression
  Test Double: source inspection
  Given docs/p0.9-deployment-checklist.md and the P138 spec
  When the known gaps section is inspected
  Then the checkpoint/outcome line names P138 as the atomic commit bridge
  And it does not say a crash between outcome insert and checkpoint write can leave a duplicate-able node
