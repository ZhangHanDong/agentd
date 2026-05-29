spec: task
name: "Engine run loop + deliver_event"
tags: [core, mvp, p0, engine]
---

## Intent

The engine that ties everything together: `Engine::execute` drives a run from
its start node through handlers to a terminal (or a park/fail), and
`Engine::deliver_event` resumes a parked run when its event arrives. This is the
attractor traversal with the Engine Execution Model invariants (D8): checkpoint
after every node, goal_gate gating at terminal transitions (D8a), retry bound
(D8c), and replay-safe event delivery.

## Decisions

- `Engine::new(graph, registry, ports, workflow_sha)`; `execute(run_id) -> RunProgress`; `deliver_event(event) -> RunProgress`.
- Loop per node: Start advances with a synthetic Success; Terminal finishes; a Regular node runs its registry handler. `HandlerStep::Done` → merge context, persist outcome, checkpoint, (maybe retry-rerun), select edge + goal_gate, advance. `HandlerStep::Park` → checkpoint + `set_current_node`, return `Parked` carrying the `ParkReason` (so the caller has the wait/review/task id).
- **Context-update reconciliation (the HandlerCtx invariant):** ctx-staged updates merge on EVERY step (run and resume, Done and Park — so a pre-park checkpoint captures them); `Outcome.context_updates` merge only on Done.
- **goal_gate (D8a):** when the selected edge targets a terminal, evaluate goal_gate over the gate nodes' latest outcomes **read from the store** (gate nodes may have completed in an earlier deliver_event segment — an in-memory map would miss them). If unmet, discard the terminal transition, synthesize a `goal_gate_unmet` Fail, and re-select once; a non-terminal recovery edge advances, otherwise the run fails.
- **Retry (D8c):** a single counter (`state.attempts`, persisted as the checkpoint's `retry_counts`, also the `attempts` map `select_next_edge` consults). A `Status::Retry` re-runs the same node while `attempts < retry_policy.max` (default 1 ⇒ a policy-less Retry routes as Fail). A global step ceiling backstops pathological graphs.
- **deliver_event replay-safety:** resolve `(run_id, node_id)` via the matching `lookup_park_by_*`; a `None` (unknown/stale/replayed id) returns `RunProgress::Ignored` (no-op, not a failure). On a match, load the checkpoint, rebuild context, resume the handler, then `Park` (re-park) or `Done` (continue the loop).

## Boundaries

### Allowed Changes

- crates/agentd-core/src/engine/{execute.rs, mod.rs, step.rs}
- crates/agentd-core/tests/engine_execute.rs

### Forbidden

- Do not build the goal_gate outcomes map from in-memory state — read latest outcomes from the store (correct across parks).
- Do not let `select_next_edge`'s attempts map and a separate counter both gate retries — one counter only.
- Do not loop unbounded: the step ceiling must backstop the run.

## Completion Criteria

Scenario: A minimal synchronous graph runs to terminal
  Test: engine_executes_minimal_three_node_graph_to_terminal
  Given a start -> conditional -> tool -> terminal graph with a runner scripted to exit 0
  When execute runs
  Then it returns RunProgress::Finished

Scenario: An outcome is persisted after each Done node
  Test: engine_persists_outcome_after_each_done_node
  Given the minimal synchronous graph
  When execute completes
  Then the store has a latest_outcome for both the conditional and the tool node

Scenario: A checkpoint is written after a node, including a park
  Test: engine_writes_checkpoint_after_each_node_including_parks
  Given a start -> wait.human -> terminal graph
  When execute runs and parks
  Then the store holds a checkpoint whose current_node is the parked node

Scenario: A run parks on wait.human then resumes to finish via deliver_event
  Test: engine_parks_on_wait_human_then_resumes_to_finish_via_deliver_event
  Given a start -> wait.human -> terminal graph routed by answer=approve
  When execute parks and deliver_event delivers HumanAnswered approve
  Then execute returns Parked and deliver_event returns Finished

Scenario: goal_gate blocks the terminal until its gate node succeeds
  Test: engine_goal_gate_blocks_terminal_until_satisfied
  Given a start -> tool(goal_gate) -> terminal graph
  When the gate tool fails the run is Failed, and when it succeeds the run is Finished

Scenario: The full canonical flow runs to completion against the fakes
  Test: engine_full_canonical_dot_runs_to_completion_with_fakes
  Given a spec(wait.human) -> impl(codergen) -> review(fan_out) -> aggregate(fan_in,goal_gate) -> terminal graph
  When execute parks at spec, then deliver_event feeds approve, the agent outcome, and three verdicts
  Then the final deliver_event returns Finished
