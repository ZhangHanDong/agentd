spec: task
name: "Worktree threading through HandlerCtx/Engine — the plumbing half (P2 C1a)"
tags: [core, engine, handler, p2, worktree]
---

## Intent

Thread an optional per-run worktree from the engine down to the spawn + tool
handlers, so a later step (C1b) can isolate each run in its own worktree without
re-touching this plumbing. This is the FIRST edit to `agentd-core` (D1 is lifted
for P2; the 84 core tests + the integration suites are the regression net that
replaces the freeze).

C1a is the PLUMBING HALF and is INERT BY DESIGN: the worktree is `Option`,
defaults to `None`, and the daemon passes `None` for now — so behavior is
unchanged and every existing test stays green untouched. C1b (allocation +
daemon wiring + reviewer-cwd semantics) makes it live; see "Sequel" below. A green
C1a means "the pipe carries water," NOT "C1 done."

## Decisions

- ADDITIVE builders, no call-site churn (HandlerCtx::new has 22 sites, Engine::new
  has 5): both keep their `new()` signatures with the worktree defaulting `None`,
  and gain a `with_worktree(...)` builder. Existing callers are unchanged.
  - `HandlerCtx` gains `worktree: Option<&'a Path>` + `with_worktree(self,
    Option<&'a Path>) -> Self` + a `worktree()` accessor.
  - `Engine` gains `worktree: Option<PathBuf>` + `with_worktree(self,
    Option<PathBuf>) -> Self`; at BOTH `HandlerCtx::new` sites it threads
    `self.worktree.as_deref()` via `with_worktree`.
- `spawn_request` gains a `worktree: &Path` parameter and sets `req.worktree` to
  it (replacing the hardcoded `"."`). `codergen` and `fan_out` pass
  `ctx.worktree().unwrap_or(Path::new("."))` — so `None` reproduces today's `"."`.
- The `tool` handler sets `RunOpts.cwd = ctx.worktree().map(Path::to_path_buf)`
  (today it is `None`); `None` worktree → `None` cwd (unchanged).
- Test support: `RecordedCall` gains `cwd: Option<PathBuf>` captured from
  `RunOpts.cwd`, so a test can assert a tool node ran in the threaded worktree.
- The daemon's `ProductionRunHost::engine(graph, sha)` passes NO worktree (stays
  `None`) in C1a — allocation is C1b. C1a changes no runtime behavior.

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
- Do not allocate, persist, or release a worktree (C1b). Do not make the daemon
  pass a real worktree (C1b).

## Out of Scope

- C1b — allocation + persistence + release of the per-run worktree, and the
  daemon actually passing it (`engine()` threading the run context to resolve the
  worktree). REQUIRED GUARD (record now so C1b can't silently degrade to `"."`):
  C1b MUST add an e2e test that a real `start_run` through `ProductionRunHost`
  records a `SpawnRequest` whose worktree is the ALLOCATED path, not `"."`. The
  `engine(&graph, &sha)` call (which takes no `run_id` today) is the exact
  chokepoint where the `.with_worktree(...)` wiring would be silently omitted —
  make that a tested chokepoint in C1b.
- Reviewer-cwd semantics: whether `fan_out`'s reviewers should run in the live
  writer worktree is OPEN (they pin a `bundle="frozen"` snapshot, and N concurrent
  readers sharing one tree needs thought). C1a only PASSES the worktree through
  `fan_out` mechanically (`None`→`"."`); the semantic decision is C1b.
- Per-run worktree lifecycle / restart handling (C1b / P1.3 re-activation).

## Completion Criteria

Scenario: a provided worktree reaches the spawned agent
  Test: engine_threads_worktree_to_spawn_request
  Given an Engine built with_worktree(Some(W)) over the in-memory fakes and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is W

Scenario: a provided worktree becomes the tool node's cwd
  Test: engine_threads_worktree_to_tool_cwd
  Given an Engine built with_worktree(Some(W)) and a graph whose codergen success leads to a tool node
  When the codergen outcome is delivered and the tool node runs
  Then the recorded tool call's cwd is Some(W)

Scenario: no worktree preserves the current "." default
  Test: engine_without_worktree_spawns_in_dot
  Given an Engine built WITHOUT with_worktree (the default None) and a graph with a codergen node
  When the run executes to the codergen park
  Then the recorded SpawnRequest's worktree is "." (behavior unchanged)
