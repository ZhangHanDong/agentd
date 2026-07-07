spec: task
name: "Port traits + in-memory fakes"
tags: [core, mvp, p0, ports]
---

## Intent

The engine and handlers depend only on trait *ports* — `AgentBackend`,
`CommandRunner`, `Store`, `MempalClient`, `Clock` — never on concrete I/O
(design §4, "no I/O in agentd-core"). This task defines those object-safe
traits and a set of in-memory fakes (`FakeBackend`, `RecordingCommandRunner`,
`InMemoryStore`, `MempalStub`, `FixedClock`) so Tasks 7–9 can build and test
handlers + the engine without a database, a tmux server, or a real agent CLI.
Per build-order D5/D6 the ports MUST exist before any handler compiles.

## Decisions

- All async traits use `#[async_trait::async_trait]` (D4) and are `Send + Sync`. `Clock` is sync.
- Every fallible port method returns `Result<_, CoreError>`. The in-memory fakes return `CoreError` directly; the real P0.2 `SqliteStore` maps its local `StoreError -> CoreError`. The trait error type is `CoreError`, NOT `StoreError` (fixes M2).
- `CommandRunner` carries its own supporting types (`RunOpts`, `CommandOutput`, `CommandError`) so the tmux backend (P0.3) reuses the exact seam.
- `Store` exposes the THREE reverse-lookup methods `deliver_event` needs: `lookup_park_by_wait_id`, `lookup_park_by_review_run`, `lookup_park_by_task_run`, each `-> Option<(RunId, NodeId)>` (fixes M4). The review-run record therefore carries `node_id`.
- `open_human_wait` / `insert_review_run` / `insert_task_run` GENERATE and return the id (wait_id: String, ReviewRunId, TaskRunId). The fake uses a monotonic counter so ids are deterministic in tests.
- `answer_human_wait` errors with `CoreError::Store` if the wait was already answered (idempotency guard).
- All three park child-rows are *replay-safe* at the lookup layer: `insert_review_verdict` is idempotent per `reviewer_id` (so `count_verdicts` counts distinct reviewers, mirroring the real store's `PRIMARY KEY (review_run_id, reviewer_id)`); `complete_task_run` closes a task run so `lookup_park_by_task_run` returns `None` afterward (mirroring `wait.human`'s close-on-answer and the real store's `task_runs.finished_at`).
- `test_support` is compiled only under `#[cfg(any(feature = "test-support", test))]`; it never ships in a release binary.
- agentd-core touches NO external store. The `MempalClient` port is an abstraction; the stub is purely in-memory and references no mempal `palace.db`.

## Boundaries

### Allowed Changes

- crates/agentd-core/src/lib.rs
- crates/agentd-core/src/ports/*.rs
- crates/agentd-core/src/test_support/*.rs
- crates/agentd-core/tests/ports_fakes.rs

### Forbidden

- Do not make any port trait non-object-safe (no generic methods, no `Self`-returning methods).
- Do not let `test_support` compile into a non-test build.
- Do not reference, open, or write mempal's `palace.db` from anywhere in agentd-core.

## Completion Criteria

Scenario: The five ports are object-safe behind dyn references
  Test: ports_traits_are_object_safe
  Given a function taking &dyn AgentBackend, &dyn CommandRunner, &dyn Store, &dyn MempalClient, and &dyn Clock
  When the fakes are passed as those trait objects and exercised
  Then it compiles and runs without a panic

Scenario: InMemoryStore round-trips a run and a node outcome
  Test: in_memory_store_round_trips_run_and_outcome
  Given an InMemoryStore with an inserted run
  When a node outcome is inserted and latest_outcome is queried for that node
  Then the stored outcome status is returned and count_attempts reflects the insert

Scenario: A human wait can be answered once, and a second answer conflicts
  Test: in_memory_store_human_wait_answer_once_then_conflict
  Given an open human wait
  When answer_human_wait is called twice for the same wait_id
  Then the first call succeeds and the second returns Err(CoreError::Store)

Scenario: lookup_park_by_wait_id resolves the parked run and node
  Test: in_memory_store_lookup_park_by_wait_id_returns_run_and_node
  Given a human wait opened for a known run_id and node_id
  When lookup_park_by_wait_id is called with the returned wait_id
  Then it returns Some((run_id, node_id)) matching the opened wait

Scenario: RecordingCommandRunner records argv and returns scripted output
  Test: recording_command_runner_records_argv_and_returns_scripted_output
  Given a RecordingCommandRunner scripted with one CommandOutput
  When run is called with a program and args
  Then the scripted output is returned and the recorded argv matches the call

Scenario: FixedClock returns the time it was set to
  Test: fixed_clock_returns_set_time
  Given a FixedClock set to a known unix time
  When now_unix is queried
  Then it returns that time, and reflects a subsequent set

Scenario: A review verdict is idempotent per reviewer
  Test: in_memory_store_review_verdict_is_idempotent_per_reviewer
  Given a review run expecting two reviewers
  When the same reviewer submits a verdict twice, then a second reviewer submits
  Then count_verdicts is 1 after the duplicate and 2 after the distinct reviewer

Scenario: A completed task run no longer parks
  Test: in_memory_store_completed_task_run_no_longer_parks
  Given an open task run that resolves via lookup_park_by_task_run
  When complete_task_run is called
  Then lookup_park_by_task_run returns None (a replayed event is a no-op)

Scenario: insert_run is idempotent first-wins
  Test: in_memory_store_insert_run_is_idempotent_first_wins
  Given a run that has been inserted and marked Finished
  When insert_run is called again for the same id
  Then the run stays Finished (a re-insert does not reset it; parity with SqliteStore)
