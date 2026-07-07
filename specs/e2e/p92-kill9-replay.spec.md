spec: task
name: "Disaster recovery — kill-9 resume + idempotent replay + the sha guard"
tags: [e2e, p0, p0.9, recovery, checkpoint]
---

## Intent

The capstone drill (P0.9 9d): prove a run survives a daemon crash. A real SIGKILL
cannot be sent to the test process, so the restart is SIMULATED by dropping the
`SqliteStore` (closing the pool) and reopening the same database file with a
fresh `ProductionRunHost` — functionally identical for the checkpoint-resume
contract, since the checkpoint is durable in SQLite. The load-bearing assertion
is not "it resumed" (the easy half) but idempotent REPLAY: the same event
delivered twice does not double-advance or duplicate. The real-SIGKILL / real-
agent drill is the deployment checklist.

## Decisions

- After a run parks and the host is dropped, reopening the same DB and delivering the park-resolving event resumes the run from its checkpoint to completion; a node completed before the restart does not re-run (`count_attempts` stays 1).
- The event log is continuous across the restart boundary: `run_parked` (pre-restart) then `run_finished` (post-restart) in strictly increasing `seq`.
- Replaying an already-resolved event returns `RunProgress::Ignored` (the park is closed → `lookup_park_by_task_run` is `None`), emits NO new event row, and creates NO duplicate outcome.
- `Checkpoint::resume_guard(current_sha, accept_change)`: the matching sha resumes; a changed sha is rejected unless `accept_change` is set (the operator's `--accept-workflow-change`).

## Boundaries

### Allowed Changes

- crates/agentd-bin/tests/**
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).

## Out of Scope

- An actual SIGKILL of a running process / real tmux agents / a real daemon restart (deployment checklist).

## Completion Criteria

Scenario: a parked run resumes from its checkpoint after a simulated restart
  Test: kill9_resume_continues_from_checkpoint
  Given a draft.dot run started to its propose_spec park, then the store dropped and reopened on the same file
  When the park-resolving outcome is delivered to a fresh host
  Then the run reaches Finished, the pre-restart node did not re-run, and the event seq is continuous across the boundary

Scenario: replaying a resolved event is ignored without duplication
  Test: replay_after_resume_is_ignored_without_duplicate
  Given a draft.dot run whose propose_spec outcome has been delivered once
  When the same AgentOutcomeSubmitted event is delivered again
  Then the second delivery returns Ignored, emits no new event, and leaves a single outcome attempt

Scenario: resume_guard gates a changed workflow sha
  Test: resume_guard_gates_a_changed_workflow_sha
  Given the checkpoint written when a run parks
  When resume_guard is evaluated with a changed sha
  Then it is rejected without accept_change and accepted with accept_change
