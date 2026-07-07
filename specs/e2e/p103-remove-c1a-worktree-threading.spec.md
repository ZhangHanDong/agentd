spec: task
name: "Remove vestigial C1a Engine-level worktree threading"
tags: [e2e, core, handler, p2, worktree, cleanup]
---

## Intent

P9 introduced an inert per-run `Engine::with_worktree` / `HandlerCtx::worktree`
plumbing layer before per-`task_run` allocation existed. P11, P99, P100, P101,
and P102 have moved the real design onto task-run allocation plus explicit
`RunContext` values: `codergen` allocates the worktree, stages `worktree` and
`task_run_id`, tools substitute from context, reviewers derive their worktree
source from context, and successful terminal completion releases the implementer
worktree. This slice removes the old C1a fallback so there is only one
authoritative implementer worktree path.

## Decisions

- Remove `Engine::with_worktree`, the `Engine.worktree` field, and the
  `HandlerCtx::with_worktree` / `HandlerCtx::worktree` API.
- Tool `${...}` substitution reads only top-level string entries from
  `RunContext`; `worktree` is available only when a previous node staged it.
- `codergen` spawns in the allocated worktree when a `WorktreeAllocator` exists,
  otherwise it spawns in `"."`; it does not read an Engine-level fallback.
- `fan_out` reviewers use the staged context `worktree` as their implementation
  source when present, otherwise `"."`; there is no hidden `ctx.worktree()`
  fallback.
- Update P9/P10 documentation to mark the C1a plumbing as superseded by the
  task-run worktree path.

## Boundaries

### Allowed Changes

- crates/agentd-core/**
- crates/agentctl/tests/**
- specs/core/p9-worktree-ctx-threading.spec.md
- specs/core/p10-tool-cmd-substitution.spec.md
- specs/core/p11-per-task-run-worktree.spec.md
- specs/e2e/**

### Forbidden

- Do not change `WorktreeAllocator::allocate` / `release` behavior.
- Do not change `task_runs.worktree_path` persistence.
- Do not make tool nodes run in the worktree cwd.
- Do not change shipped workflow DOT semantics.

## Out of Scope

- Independent reviewer worktrees are handled by P104.
- Restart reuse of in-flight worktrees.
- Any schema migration.

## Completion Criteria

Scenario: Engine and HandlerCtx expose no per-run worktree threading API
  Test: core_has_no_engine_or_handlerctx_worktree_threading
  Level: core static
  Test Double: source inspection
  Given the current agentd-core source files
  When the static cleanup test scans Engine, HandlerCtx, tool, codergen, and fan_out
  Then no `with_worktree`, `ctx.worktree`, or Engine-level worktree field remains

Scenario: codergen without allocator still spawns in dot
  Test: codergen_without_allocator_spawns_in_dot
  Level: core workflow integration
  Test Double: FakeBackend + InMemoryStore
  Given an Engine with no WorktreeAllocator and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is "."

Scenario: allocated worktree still reaches downstream tool variables through context
  Test: tool_cmd_substitutes_worktree_and_context_var
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given a codergen node stages an allocated worktree and task_run_id before a tool node runs "verify --code ${worktree} --run ${task_run_id}"
  When the codergen outcome is delivered and the tool node runs
  Then the recorded tool call's args contain the allocated worktree and task_run_id, with no literal "${" remaining

Scenario: reviewers still derive from the staged allocated worktree
  Test: execute_dot_reviewers_receive_independent_worktrees
  Level: workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake snapshot WorktreeAllocator
  Given execute.dot on the real Engine with a fake allocator returning "/tmp/agentd-task-wt" for implement and distinct reviewer snapshot paths
  When implement succeeds and the review fan-out parks
  Then reviewer SpawnRequests use independent worktrees copied from the implementer worktree

Scenario: historical C1a specs point to the task-run replacement
  Test: worktree_threading_specs_mark_c1a_superseded
  Level: spec static
  Test Double: source inspection
  Given specs/core/p9-worktree-ctx-threading.spec.md and specs/core/p10-tool-cmd-substitution.spec.md
  When the static cleanup test reads those files
  Then both mention that Engine-level C1a worktree threading is superseded by the task-run worktree path
