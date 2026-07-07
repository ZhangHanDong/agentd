spec: task
name: "Per-task_run worktree allocation — connect the design-faithful worktree (P2 C1' R3a)"
tags: [core, store, bin, handler, p2, worktree]
---

> STATUS: COMPLETE FOR R3a — this mechanism contract is implemented and verified.
> Activation is covered by P99-P104: daemon allocator injection, execute.dot
> `${worktree}` consumption, branch publication, reviewer snapshots, and
> worktree release/cleanup are follow-on contracts rather than p11 concerns.
> The vestigial C1a threading cleanup is handled by P103.

## Intent

Make the worktree real (R1 reverted the wrong cwd model; R2 restored `${...}`
substitution). R3a is the mechanism contract: the `WorktreeAllocator` port, the
codergen reorder behind it, `set_task_run_worktree`, and the builder seam. When
an allocator is injected, codergen allocates a per-task_run worktree, spawns the
agent into it, persists `task_runs.worktree_path`, and stages it so a downstream
tool resolves `${worktree}`. Proven by fake-allocator tests.

R3a is complete. The current daemon/workflow activation is handled by later
contracts: P99 injects the real pool into the production host, P100 publishes the
implementer worktree branch before PR creation, P101 releases worktrees after
terminal success, P104 gives reviewers independent snapshot worktrees, and
P106/P107 cover failed-run cleanup and maintenance hardening. This p11 spec stays
focused on the reusable core/store mechanism.

Load-bearing assumption (VERIFIED): a value codergen stages into the context
survives the park/resume — `step_once` merges staged updates into `state.context`
BEFORE `write_checkpoint`, and `deliver_event` restores `state.context` from
`context_snapshot`. R2's integration test already proved this end-to-end
(codergen-staged `task_run_id` resolved by a tool after `deliver_event`); the
worktree stages identically.

## Decisions (R3a)

- A new CORE port `WorktreeAllocator` (`ports/`): `async fn allocate(&self, key:
  &str) -> Result<PathBuf, CoreError>`. (`WorktreeAllocator`, not
  `WorktreeProvider`, to avoid colliding with agentd-tmux's existing
  `WorktreeProvider`; agentd-tmux/agentd-bin supply an impl backed by
  `GitWorktreeProvider`.)
- INJECTION via a `HandlerCtx` builder, NOT a `Ports` field — `Engine` and
  `HandlerCtx` gain `with_worktree_allocator(Option<&dyn WorktreeAllocator>)`
  (default `None`), exactly like C1a's `with_worktree`. This keeps the 22
  `HandlerCtx::new` + 5 `Engine::new` literal sites unchanged (a `Ports` field
  would churn every literal). Production activation opts in through later specs;
  core tests can still use `None` to preserve the old `"."` behavior.
- codergen REORDER (the fix the frozen core blocked, now D1-lifted): today it
  spawns THEN `insert_task_run`. New order — `insert_task_run` → `task_run_id` →
  `allocate(task_run_id)` → `W` (if an allocator is injected; else `"."`) →
  `spawn_request(role, prompt, W)` (agent runs in W) → `store.set_task_run_worktree
  (task_run_id, W)` → stage `context["worktree"] = W` (+ existing `task_run_id`) →
  Park. The agent is isolated in W; the worktree reaches downstream tools via the
  context.
- A new additive `Store` method `set_task_run_worktree(task_run_id, path)` writes
  `task_runs.worktree_path` (the existing column). Additive — no migration
  (Foundation B's harness guards future ones; this column already exists).
- `allocate → spawn → set_task_run_worktree` is NON-ATOMIC best-effort: a spawn
  failure can orphan a freshly-allocated worktree, and a persist failure after a
  successful spawn leaves W unrecorded. Acceptable for the MVP (same non-atomicity
  class as the logged checkpoint/outcome gap); stated, not silent. Cleanup of
  orphans is R3b's GC concern.
- C1a SUPERSESSION (stated, not silent): per-task_run context-staging supersedes
  C1a's Engine-level `with_worktree` / `ctx.worktree()` — codergen now allocates
  locally and does NOT read the Engine worktree. P103 removes that vestigial
  threading; the task-run worktree path is the only live core channel.
  (Worktree mechanism has now shifted three times: per-run→per-task_run,
  cwd→arg, Engine-threaded→context-staged.)

## Boundaries

### Allowed Changes

- crates/agentd-core/**
- crates/agentd-store/**
- specs/core/**

### Forbidden

- Do not broaden p11 beyond the core/store mechanism that R3a verifies; daemon
  activation, reviewer snapshots, PR publication, and cleanup are documented in
  P99-P104/P106/P107.
- Do not add a new migration: `task_runs.worktree_path` already exists.

## Out of Scope (activation follow-ons)

- Activation is covered by P99-P104: real allocator injection, execute.dot's
  `${worktree}` use, branch publication, reviewer snapshots, and release are
  follow-on contracts, not p11 acceptance criteria.
- REVIEWER worktrees: P104 resolves this by keying reviewer snapshots as
  `review-${review_run_id}-${reviewer_id}` and snapshotting from the implementer
  worktree.
- `open_pr` from the worktree: the PR helper needs the implementer's work on a
  pushed branch; P100 handles this by publishing W to `agentd/${task_run_id}`.
- RESTART: a persisted `worktree_path` points to a worktree P1.3 boot-GC would
  delete on restart; reuse-if-exists vs reallocate vs spare-in-flight is R3b. (Do
  NOT wire boot-GC in R3a.)
- BOOT-GC NAMING: P1.3's boot-GC filter matches `wt-<digits>-<digits>`; the real
  R3b allocator names worktrees by `task_run_id` (a ULID), which that filter won't
  match — R3b must either name them to fit the filter or update the filter (with
  its foreign-preservation test).
- Restart reuse for in-flight reviewer snapshots after P104.

## Completion Criteria (R3a)

Scenario: the implementer agent is spawned in an allocated per-task_run worktree
  Test: codergen_spawns_in_allocated_worktree
  Level: core integration
  Test Double: Fake WorktreeAllocator + FakeBackend + InMemoryStore
  Given an Engine with a fake WorktreeAllocator that allocates a known path and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is the allocated path (not ".") and task_runs.worktree_path is that path

Scenario: a downstream tool resolves ${worktree} to the allocated worktree
  Test: tool_resolves_allocated_worktree_via_context
  Level: core integration
  Test Double: Fake WorktreeAllocator + FakeBackend + RecordingCommandRunner + InMemoryStore
  Given the codergen node staged the allocated worktree and a following tool node runs "verify --code ${worktree}"
  When the codergen outcome is delivered and the tool runs
  Then the recorded tool call's args contain the allocated worktree path

Scenario: set_task_run_worktree persists the path for open task lookup
  Test: set_task_run_worktree_persists_path
  Level: store integration
  Test Double: real SqliteStore on tempfile
  Given a SqliteStore with an open task_run and the path "/tmp/wt-task"
  When set_task_run_worktree writes "/tmp/wt-task"
  Then find_open_task_run returns that same worktree path

Scenario: with no allocator the implementer falls back to "." (behavior preserved)
  Test: codergen_without_allocator_spawns_in_dot
  Level: core integration
  Test Double: FakeBackend + InMemoryStore
  Given an Engine with NO WorktreeAllocator injected and a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is "." (today's behavior, all existing tests unchanged)

Scenario: allocator failure aborts codergen before spawning an agent
  Test: codergen_allocator_failure_does_not_spawn
  Level: core integration
  Test Double: failing Fake WorktreeAllocator + FakeBackend + InMemoryStore
  Given an Engine with a fake WorktreeAllocator that returns an error and a graph with a codergen node
  When the run reaches codergen
  Then execute returns an error mentioning the allocator failure
  And no SpawnRequest is recorded
