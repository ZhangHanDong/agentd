spec: task
name: "Production RunHost contract — draft.dot E2E over a real store + the emit point"
tags: [e2e, p0, p0.9, runhost, emit]
---

## Intent

Prove the production `RunHost` (P0.9 9a) drives a real workflow end-to-end over a
real `SqliteStore`, using a scriptable in-process agent that submits outcomes
through `mcp_server::dispatch` (the same tool layer a real agent uses, minus the
rmcp wire). It also lands the P0.7-deferred emit point: one event row per
state-changing `RunProgress`. This is the offline contract test; real agents /
tmux / rmcp are the deployment checklist.

## Decisions

- The host's `start_run(run_id)` resolves the run's graph from `runs.workflow_path` and executes to the first park (emitting `run_parked`); `dispatch("submit_outcome", …)` resolves the open task via `find_open_task_run` and advances the run (emitting `run_finished` on completion).
- The emit point writes ONE row per STATE-CHANGING `RunProgress`: `Parked`→`run_parked`, `Finished`→`run_finished`, `Failed`→`run_failed`; `Ignored` emits nothing. Payloads are compact JSON.
- Consecutive same-node re-parks are DEDUPED (P1 re-park-noise gap): a `Parked` progress emits nothing — no event row AND no live broadcast — when the run's most recent event is already a `run_parked` with the same node payload. A fan-out review re-parks at the same node per non-final verdict (e.g. `majority_pass` over 3 reviewers re-parks at `review` after the 1st pass); only the FIRST park at a node emits. Distinct-node parks and ALL terminals (`run_finished`/`run_failed`) always emit. This keeps both the durable log and the SSE tail/replay free of same-node duplicates. (`event_repo::last` supplies the most-recent-event check.)
- `draft.dot` walks: `start_run` parks at `propose_spec`; submitting its success drives `lint_spec`+`push_draft` to `done` (Finished). The store, checkpoints, and event rows all persist to a real SQLite file.
- A replayed `submit_outcome` for an already-closed task is rejected (its open task is gone) and emits NO new event row.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- crates/agentd-store/**
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not pull agentd-store into agentd-surface (P0.7 D2 — the production host lives in agentd-bin).

## Out of Scope

- Real agents / real tmux spawn / the rmcp stdio wire (deployment checklist).
- execute.dot's review fan-out (needs the in-process RunProgress to learn review_run_id) — the kill-9 drill and 9b/9c cover the daemon paths.

## Completion Criteria

Scenario: the production host drives draft.dot to done over a real store
  Test: production_runhost_drives_draft_dot_to_done
  Given a production RunHost over a real SqliteStore with a recorded draft.dot run
  When start_run parks at propose_spec and the scriptable agent submits its success via dispatch
  Then the run reaches status "finished" and the emitted events are run_parked then run_finished in increasing seq

Scenario: a replayed submit for a closed task is rejected with no new event
  Test: production_runhost_replayed_submit_is_rejected_without_new_event
  Given a draft.dot run that has been driven to done
  When submit_outcome is dispatched again for the already-completed propose_spec node
  Then the dispatch returns an error and no additional event row is emitted

Scenario: the production host drives execute.dot (fan-out review) to done
  Test: production_runhost_drives_execute_dot_to_done
  Given a production RunHost with a recorded execute.dot run
  When start_run parks at implement, its success is submitted, and three reviewers submit pass verdicts (via submit_review, learning review_run_id from the store)
  Then the run reaches status "finished" and the emitted events park at implement and review before run_finished

Scenario: a same-node re-park is deduped to a single run_parked
  Test: production_runhost_dedupes_same_node_reparks
  Given an execute.dot run driven to its review fan-out park
  When one reviewer submits a non-final pass verdict that re-parks at the same review node, then the remaining reviewers complete it
  Then exactly one run_parked event exists for the review node and the run reaches finished

Scenario: events_from for an unknown run is empty
  Test: production_runhost_events_from_unknown_run_is_empty
  Given a production RunHost over a real SqliteStore
  When events_from is called for a run that does not exist
  Then it returns no events
