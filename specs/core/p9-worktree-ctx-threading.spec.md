spec: task
name: "Worktree threading through HandlerCtx/Engine — historical C1a, superseded"
tags: [core, engine, handler, p2, worktree]
---

## Intent

Historical note: P9 originally threaded an optional per-run worktree from
`Engine` into `HandlerCtx`. That C1a API is now superseded by the task-run worktree path:
codergen allocates per `task_run_id`, stages `context["worktree"]`,
and downstream tool/fan_out handlers read the staged context value.

The current invariant is intentionally narrower and easier to reason about:
there is no Engine-level or HandlerCtx-level worktree field. Without an injected
`WorktreeAllocator`, codergen still spawns in `"."`; with one, the allocated
task-run path reaches `SpawnRequest.worktree` and the run context.

## Decisions

- P103 removes the old additive `with_worktree` builders, the Engine field, and
  the HandlerCtx accessor. The task-run worktree path is the only live worktree
  channel inside core.
- `spawn_request` gains a `worktree: &Path` parameter and sets `req.worktree` to
  it (replacing the hardcoded `"."`). `codergen` passes the allocated task-run
  path, or `"."` when no allocator is injected. `fan_out` uses the staged
  `context["worktree"]` as the implementation source, or `"."` when absent.
- Tool nodes do NOT take the worktree as cwd. They run in the daemon cwd and a
  code tool receives the worktree as an explicit `--code ${worktree}` argument
  via context-variable substitution.
- Test support: `RecordedCall` gains `cwd: Option<PathBuf>` captured from
  `RunOpts.cwd`.
- The daemon injects a `WorktreeAllocator` for active workflows; allocation is
  keyed by `task_run_id`, not by the outer workflow run.

## Boundaries

### Allowed Changes

- crates/agentd-core/** (the first P2 core edit — D1 lifted; engine/handler
  plumbing + test_support)
- crates/agentd-bin/** (only the `engine()` call site, still passing `None`)
- specs/core/**

### Forbidden

- Do not change the BEHAVIOR of any existing test — they must stay green WITHOUT
  edits (the additive-builder guarantee). A diff to an existing test's
  expectations means the change was not behavior-preserving.
- Do not reintroduce Engine-level or HandlerCtx-level worktree threading.

## Out of Scope

- Independent reviewer worktrees are handled by P104; they snapshot from the
  implementer's staged task-run worktree instead of reintroducing Engine-level
  worktree threading.
- Restart reuse remains out of scope; startup GC can reclaim leftover pool
  worktrees.

## Completion Criteria

Scenario: Engine and HandlerCtx expose no per-run worktree threading API
  Test: core_has_no_engine_or_handlerctx_worktree_threading
  Given the core engine and handler source files
  When the P103 regression test scans them
  Then no Engine-level worktree field, with_worktree builder, HandlerCtx accessor, or ctx.worktree fallback remains

Scenario: no worktree preserves the current "." default
  Test: codergen_without_allocator_spawns_in_dot
  Given an Engine built without a WorktreeAllocator and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is "." (behavior unchanged)
