spec: task
name: "Per-task_run worktree allocation — connect the design-faithful worktree (P2 C1' R3a, DRAFT)"
tags: [core, store, bin, handler, p2, worktree, draft]
---

> STATUS: DRAFT — design pending an advisor review pass before implementation
> (R3 is the deepest C1 piece; the team gates coding on a reviewed spec). The
> R3b forks (reviewer worktrees, open_pr-from-worktree, restart, removing the
> vestigial C1a threading) are scoped OUT here with their open questions stated.

## Intent

Make the worktree real (R1 reverted the wrong cwd model; R2 restored `${...}`
substitution). R3a allocates a per-task_run worktree for the IMPLEMENTER agent
(codergen), spawns the agent INTO it, persists it on `task_runs.worktree_path`
(the column Foundation B left), and stages it into the run context so a later
code tool resolves `${worktree}` to it — closing the loop the design intends
(`agent-spec lifecycle … --code ${worktree}`).

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
  would churn every literal). The daemon opts in; tests default to `None`.
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
- Migrate `execute.dot`'s `verify_lifecycle` `--code .` → `--code ${worktree}` so
  the spec-acceptance gate verifies the IMPLEMENTER's worktree (R2's substitution
  resolves it from the staged context). draft.dot needs no change (its tools read
  `.agentd/run/` only; no `--code`).
- C1a SUPERSESSION (stated, not silent): per-task_run context-staging supersedes
  C1a's Engine-level `with_worktree` / `ctx.worktree()` — codergen now allocates
  locally and does NOT read the Engine worktree. C1a's threading is left INERT in
  R3a (daemon passes it `None`); its removal is R3b. (Worktree mechanism has now
  shifted three times: per-run→per-task_run, cwd→arg, Engine-threaded→context-
  staged.)

## Boundaries

### Allowed Changes

- crates/agentd-core/** (the `WorktreeAllocator` port, codergen reorder, the
  HandlerCtx/Engine builder, the Store-trait method)
- crates/agentd-store/** (the `set_task_run_worktree` impl)
- crates/agentd-bin/** (inject the allocator + the daemon wiring)
- crates/agentd-tmux/** (a `WorktreeAllocator` impl over `GitWorktreeProvider`)
- workflows/execute.dot
- specs/core/**, specs/workflow/p81-execute-dot.spec.md

### Forbidden

- Do not allocate worktrees for reviewers / `fan_out` (R3b — reviewers are not
  task_runs; see Out of Scope).
- Do not change `open_pr` to operate from the worktree (R3b — needs a branch).
- No new migration: `task_runs.worktree_path` already exists.

## Out of Scope (R3b — open questions, parked deliberately)

- REVIEWER worktrees: design line 168 says "each reviewer gets its own worktree
  pwd", but reviewers are spawned by `fan_out` and are NOT task_runs — keyed by
  what? sharing/forking the implementer's W? An open fork for R3b.
- `open_pr` from the worktree: `gh pr create` needs the implementer's work on a
  pushed branch; how the agent's commits in W reach the PR is R3b.
- RESTART: a persisted `worktree_path` points to a worktree P1.3 boot-GC would
  delete on restart; reuse-if-exists vs reallocate vs spare-in-flight is R3b. (Do
  NOT wire boot-GC in R3a.)
- REMOVING C1a's now-vestigial Engine `with_worktree` threading — R3b cleanup.

## Completion Criteria (R3a)

Scenario: the implementer agent is spawned in an allocated per-task_run worktree
  Test: codergen_spawns_in_allocated_worktree
  Given an Engine with a fake WorktreeAllocator that allocates a known path and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is the allocated path (not ".") and task_runs.worktree_path is that path

Scenario: a downstream tool resolves ${worktree} to the allocated worktree
  Test: tool_resolves_allocated_worktree_via_context
  Given the codergen node staged the allocated worktree and a following tool node runs "verify --code ${worktree}"
  When the codergen outcome is delivered and the tool runs
  Then the recorded tool call's args contain the allocated worktree path

Scenario: with no allocator the implementer falls back to "." (behavior preserved)
  Test: codergen_without_allocator_spawns_in_dot
  Given an Engine with NO WorktreeAllocator injected and a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is "." (today's behavior, all existing tests unchanged)
