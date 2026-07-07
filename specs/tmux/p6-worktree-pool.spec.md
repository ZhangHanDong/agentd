spec: task
name: "Worktree pool — per-spawn isolated worktrees via a backend decorator (§7.3 P1.3)"
tags: [tmux, worktree, p1, isolation, backend]
---

## Intent

Give each spawned agent its own isolated git worktree so two runs reaching
`implement` (codergen) concurrently do not edit the same tree. Today the frozen
core hardcodes `SpawnRequest.worktree = "."` (`handler/mod.rs`) and the daemon
serializes nothing — concurrent `POST /runs` / `deliver` execute concurrently —
so real (TmuxBackend) agents collide in the repo root.

The clean shape (allocate per `task_run`, persist `worktree_path`, release on
`complete_task_run`) needs core to thread a worktree through the spawn path, but
core is FROZEN (D1) and `AgentBackend` has only `spawn` (no `kill`/lifecycle).
So this lands the isolation half WITHOUT touching core: a backend DECORATOR that
allocates a fresh isolated worktree per spawn, plus boot-time GC. The per-
`task_run` lifecycle (persistence + prompt release) is deferred to the P2 core
extraction (see Out of Scope) — a deliberate, documented partial.

## Decisions

- A `WorktreeProvider` seam abstracts the git worktree ops: `create(name) ->
  PathBuf`, `remove(path)`, `list() -> Vec<PathBuf>`. `GitWorktreeProvider`
  shells `git worktree add/remove/list`; tests drive an in-memory fake (seam +
  fake), so the pool's allocation/GC logic is unit-testable with no git/tmux.
- A `WorktreePool` allocates a FRESH worktree per request, named
  `wt-{pid}-{counter}` (a process-id prefix + a lock-free atomic counter). No
  reuse — so concurrent allocations are inherently DISTINCT (isolation comes from
  fresh allocation, not from a lock). An explicit reuse-lock is intentionally NOT
  built: reuse needs a "this worktree is now free" signal, i.e. the per-`task_run`
  release lifecycle that D1 + the spawn-only `AgentBackend` defer to P2.
- A `PooledBackend<B: AgentBackend>` decorator implements `AgentBackend`: on
  `spawn(req)`, if `req.worktree` is the `"."` auto-sentinel (what frozen core
  passes), allocate a pool worktree and override `req.worktree` before delegating
  to the inner backend; any other (explicit) worktree passes through unchanged.
  The engine, handlers, and core stay untouched (D1). The decorator + provider
  are built and unit-tested, but WIRING into `build_production_host` is DEFERRED
  (see Out of Scope) — activating under D1 would strand the agent's work.
- Cleanup is BOOT-GC only: at daemon start, `WorktreePool::gc_on_boot` removes
  every leftover pool worktree (`provider.list` → `remove` each). Nothing pool-
  owned survives a restart — in-flight runs re-spawn fresh on resume from
  checkpoint — so reclaiming all leftovers at boot is correct. No continuous /
  periodic / liveness GC (the trait has no `kill`; intra-process release needs
  the missing lifecycle).
- The `list` filter feeding boot-GC's `git worktree remove --force` is TIGHT: a
  pure `pool_worktrees`/`is_pool_name` keeps only the exact `wt-<digits>-<digits>`
  shape the pool mints. A foreign worktree — even one a human named `wt-feature`
  — is never returned, so the `--force` delete can only touch trees the pool
  created. (Dir-name match, not a path prefix — robust to the canonical/symlinked
  paths git reports, e.g. macOS /tmp → /private/tmp.)

## Boundaries

### Allowed Changes

- crates/agentd-tmux/**
- crates/agentd-bin/** (wiring `PooledBackend` + boot-GC into the daemon)
- specs/tmux/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1) — including
  `SpawnRequest`/`spawn_request`/the `AgentBackend` trait.
- Do not add a store dependency to agentd-surface (D2).

## Out of Scope

- ACTIVATION — wiring `PooledBackend` into the daemon `serve()` path. Deferred:
  under D1 the downstream `tool` nodes (`verify_lifecycle`'s goal_gate, `open_pr`)
  run in the daemon cwd, not the agent's worktree (the frozen tool handler leaves
  `RunOpts.cwd = None`), so isolating the agent into a worktree the gate + PR
  never read would trade a collision for STRANDED work. Needs a worktree→pipeline
  bridge — the same P2 core change as the per-`task_run` lifecycle below. The
  mechanism + tests land now; activation is one wrapper line when the bridge does.
  BRIDGE SCOPE: the cwd-split affects EVERY tool+agent workflow, not just
  execute.dot's verify/PR — bootstrap.dot (`scaffold`/`lint` in cwd vs `discover`
  in W) and refactor-only.dot have the same shape, so the bridge must point all
  tool nodes at the run's worktree, not special-case one graph.
- Per-`task_run` `worktree_path` persistence + release on `complete_task_run`:
  the backend can't correlate a spawn to its task_run under frozen core (spawn
  precedes `insert_task_run`; `SpawnRequest` carries no run/node id). Needs the
  P2 core extraction.
- A reusable bounded pool + explicit allocation lock + concurrency cap/semaphore:
  all need the per-spawn release lifecycle above.
- Continuous / periodic / process-liveness GC (boot-GC only this pack).
- The real `git worktree` shell-out details (integration, not unit-tested; the
  fake provider covers the pool logic).

## Completion Criteria

Scenario: fresh allocation yields distinct isolated worktrees
  Test: pool_allocates_distinct_worktrees
  Given a WorktreePool over an in-memory provider
  When allocate is called twice
  Then the two returned worktree paths are different

Scenario: concurrent allocations are all distinct
  Test: pool_concurrent_allocations_are_distinct
  Given a WorktreePool over an in-memory provider
  When many allocations run concurrently
  Then every returned worktree path is unique

Scenario: the decorator overrides the auto worktree at spawn
  Test: pooled_backend_overrides_auto_worktree
  Given a PooledBackend wrapping a recording inner backend
  When spawn is called with the "." auto worktree
  Then the inner backend receives a pool worktree path, not "."

Scenario: the decorator respects an explicit worktree
  Test: pooled_backend_passes_through_explicit_worktree
  Given a PooledBackend wrapping a recording inner backend
  When spawn is called with an explicit non-"." worktree
  Then the inner backend receives that same worktree unchanged

Scenario: boot-GC removes leftover pool worktrees
  Test: boot_gc_removes_leftover_worktrees
  Given a provider holding several leftover pool worktrees
  When gc_on_boot is called
  Then the provider lists no remaining worktrees

Scenario: a provider failure at allocation is surfaced
  Test: pool_allocate_provider_error_is_surfaced
  Given a WorktreePool whose provider fails to create a worktree
  When allocate is called
  Then it returns an error

Scenario: boot-GC's filter spares foreign worktrees
  Test: pool_worktrees_keeps_only_pool_dirs_preserving_foreign
  Given git worktree porcelain output with the main tree, a wt-<pid>-<n> pool worktree, and a human's named worktree
  When the pool-worktree filter parses it
  Then only the pool worktree is returned, so boot-GC's force-remove never touches the main tree or the foreign one

Scenario: the pool-name match is tight, not a loose prefix
  Test: is_pool_name_is_tight_not_a_loose_prefix
  Given candidate worktree directory names
  When is_pool_name classifies them
  Then only the exact wt-<digits>-<digits> shape matches and a name like wt-feature does not
