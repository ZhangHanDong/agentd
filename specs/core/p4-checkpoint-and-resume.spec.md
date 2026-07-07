spec: task
name: "Checkpoint persistence + resume sha policy"
tags: [core, mvp, p0, workflow]
---

## Intent

After every node the engine writes a durable `Checkpoint` so a `kill -9` can be
resumed. The checkpoint round-trips through JSON with deterministic key order,
is written atomically (temp file + rename), and is gated on resume by the
workflow's sha so a changed `.dot` cannot be silently resumed against stale
state (design §3.3 resume policy).

## Decisions

- `Checkpoint { run_id, current_node, completed_nodes: Vec<NodeId>, retry_counts: BTreeMap<NodeId,u32>, context_snapshot: RunContext, workflow_sha: String }`
- `retry_counts` is a BTreeMap (not HashMap) so serialized key order is stable
- Checkpoint timing vs. a Park (D8/staged-context): the engine takes the checkpoint AFTER a handler returns, with `context_snapshot` reflecting everything the handler staged into the context before returning Park — nothing in-flight is lost across the park boundary
- `write_atomic(path)` writes `<path>.tmp` then renames over `path` (POSIX-atomic)
- `load(path) -> Result<Checkpoint, CoreError>`
- `resume_guard(&self, current_sha, accept_change) -> Result<(), CoreError>`: sha match → Ok; mismatch + !accept → `Err(CoreError::WorkflowShaChanged)`; mismatch + accept → Ok with a `tracing::warn!`
- This is the only place agentd-core touches the filesystem; it is local checkpoint state, not an external port

## Boundaries

### Allowed Changes

- crates/agentd-core/src/engine/checkpoint.rs
- crates/agentd-core/src/engine/mod.rs
- crates/agentd-core/tests/checkpoint.rs

### Forbidden

- Do not resume across a changed workflow_sha without the explicit accept flag.
- Do not use a HashMap for retry_counts (non-deterministic key order).

## Completion Criteria

Scenario: A checkpoint round-trips through JSON
  Test: checkpoint_serialize_round_trips_through_json
  Given a checkpoint with completed nodes and retry counts
  When it is serialized to JSON and deserialized back
  Then the result equals the original

Scenario: write_atomic persists via a temp file and rename
  Test: checkpoint_write_is_atomic_via_temp_rename
  Given a checkpoint and a target path in a temp dir
  When write_atomic runs
  Then the target file exists and loads back equal to the original
  And no leftover .tmp file remains

Scenario: The snapshot includes context staged before a park
  Test: checkpoint_snapshot_includes_context_staged_before_park
  Given a run context into which a handler staged an answer key before parking
  When a checkpoint is taken from that context and reloaded
  Then the reloaded context still carries the staged answer key

Scenario: Resume succeeds when the workflow sha matches
  Test: checkpoint_resume_succeeds_when_sha_matches
  Given a checkpoint with workflow_sha "abc"
  When resume_guard is called with current sha "abc"
  Then it returns Ok

Scenario: Resume errors when the sha changed and no accept flag is set
  Test: checkpoint_resume_errors_when_sha_changed_without_accept_flag
  Given a checkpoint with workflow_sha "abc"
  When resume_guard is called with current sha "xyz" and accept_change=false
  Then it returns Err(WorkflowShaChanged)

Scenario: Resume proceeds when the sha changed but accept flag is set
  Test: checkpoint_resume_proceeds_with_accept_flag_and_logs_warning
  Given a checkpoint with workflow_sha "abc"
  When resume_guard is called with current sha "xyz" and accept_change=true
  Then it returns Ok
