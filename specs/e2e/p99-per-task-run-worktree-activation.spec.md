spec: task
name: "Per-task_run worktree activation — daemon-injected allocator + execute bridge (P2 C1' R3b1)"
tags: [e2e, daemon, tmux, workflow, p2, worktree]
---

## Intent

P11/R3a made the per-`task_run` worktree mechanism real but deliberately INERT:
core can accept a `WorktreeAllocator`, `codergen` can allocate and persist
`task_runs.worktree_path`, and downstream tools can substitute `${worktree}` from
context, but the daemon still injects no allocator and shipped workflows still
verify `"."`.

This R3b1 slice activates the smallest coherent production path for
`execute.dot`: the production host can inject a real allocator into every
per-call `Engine`, the tmux worktree pool can act as that allocator using the
`task_run_id` as the worktree name key, `execute.dot` verifies the allocated
code tree via `${worktree}`, and reviewers spawned after implementation receive
the implementation content. P104 later moves reviewers from sharing that same
worktree to independent snapshot worktrees. The old `PooledBackend` decorator
remains backward compatible but is no longer the activation path.

This is still not the full R3b finish line. Branch publication / `open_pr` from
the worktree is covered by P100, successful-run worktree release is covered by
P101, C1a's vestigial Engine-level worktree threading cleanup is covered by
P103, and independent reviewer snapshots are covered by P104.

## Decisions

- `ProductionRunHost` gains an optional `WorktreeAllocator` field and threads it
  into `Engine::with_worktree_allocator(...)` in the one `engine(...)` factory.
  `build_production_host` installs a `GitWorktreeProvider`-backed
  `WorktreePool` so the shipped daemon path is active. No surface API change:
  this is daemon composition, not an HTTP/MCP contract.
- `WorktreePool` implements the core `WorktreeAllocator` port by allocating a
  deterministic pool-owned name derived from the `task_run_id` key. The legacy
  `allocate()` counter path stays for the already-shipped `PooledBackend` tests.
- Task-keyed pool names are tight and explicit: `wt-task-tr_<ULID>`. Boot-GC's
  parser may keep recognizing legacy `wt-<pid>-<n>` names, but it must also
  recognize only valid task-keyed names and must not match loose human names like
  `wt-task-feature`.
- Boot-GC remains startup-only and removes pool-owned legacy, task-keyed, and
  reviewer-keyed worktrees. In-flight restart reuse remains out of scope; the
  current policy is to reclaim leftovers and let replay reallocate.
- `fan_out` uses the staged string context variable `worktree` when present as
  the implementation source. P104 allocates independent reviewer snapshots from
  that source; if no allocator/context value is present, it falls back to the
  staged worktree or `"."`.
- `execute.dot` migrates only the code-verification argument:
  `agent-spec lifecycle .agentd/run/frozen.spec.md --code ${worktree} ...`.
  Runtime-state paths such as `.agentd/run/frozen.spec.md` remain daemon-cwd
  paths; tool cwd stays unchanged by design.

## Boundaries

### Allowed Changes

- crates/agentd-bin/**
- crates/agentd-core/**
- crates/agentd-tmux/**
- crates/agentctl/**
- workflows/execute.dot
- specs/e2e/**
- specs/workflow/p81-execute-dot.spec.md

### Forbidden

- Do not add `agentd-store` to `agentd-surface`.
- Do not make tool nodes run with cwd set to the worktree; `${worktree}` is an
  explicit argument bridge, not a cwd bridge.
- Do not silently fall back to `"."` after allocator failure.
- Do not wire the old `PooledBackend` decorator as the R3b activation path.

## Out of Scope

- `open_pr` branch semantics are handled by P100, not by this activation slice.
- Independent reviewer worktrees are handled by P104.
- Successful-run worktree release is handled by P101. Releasing at
  `complete_task_run` remains forbidden because downstream verify/review/publish
  nodes still need the worktree.
- Removing C1a's Engine-level `with_worktree` / `ctx.worktree()` fallback is
  handled by P103.
- Migrating other shipped PR workflows (`docs-only`, `bugfix-rapid`,
  `refactor-only`) is handled by P102. `bootstrap` and `draft` remain outside
  this activation slice.

## Completion Criteria

Scenario: the worktree pool implements task-keyed allocation
  Test: pool_allocates_task_keyed_worktree_via_allocator_port
  Level: adapter unit
  Test Double: in-memory WorktreeProvider
  Given a WorktreePool over an in-memory provider and task_run_id "tr_0123456789ABCDEFGHJKMNPQRS"
  When the core WorktreeAllocator port allocates for that task_run_id
  Then the provider creates the worktree name "wt-task-tr_0123456789ABCDEFGHJKMNPQRS"

Scenario: task-keyed boot-GC matching is tight
  Test: pool_worktrees_keeps_task_keyed_names_preserving_foreign
  Level: adapter unit
  Test Double: porcelain fixture
  Given git worktree porcelain output with a valid task-keyed pool worktree and a human worktree named "wt-task-feature"
  When the pool-worktree filter parses it
  Then only the valid task-keyed pool worktree is returned

Scenario: ProductionRunHost threads an allocator into execute.dot
  Test: production_runhost_execute_uses_injected_worktree_allocator
  Level: e2e contract
  Test Double: FakeBackend + RecordingCommandRunner + real SqliteStore + fake WorktreeAllocator
  Given a ProductionRunHost with a fake WorktreeAllocator returning "/tmp/agentd-task-wt" and an execute.dot run
  When the run parks at implement and the implementer outcome is submitted
  Then the recorded verify_lifecycle command uses the argument pair "--code" and "/tmp/agentd-task-wt" and the open task assignment exposes that worktree

Scenario: ProductionRunHost propagates allocator failure before verify
  Test: production_runhost_allocator_failure_stops_execute_before_verify
  Level: e2e contract
  Test Double: FakeBackend + RecordingCommandRunner + real SqliteStore + failing WorktreeAllocator
  Given a ProductionRunHost with a fake WorktreeAllocator that returns an error and an execute.dot run
  When the run reaches implement during start_run
  Then start_run returns the allocator error and no verify_lifecycle command is recorded

Scenario: execute.dot reviewers receive independent snapshots of the allocated worktree
  Test: execute_dot_reviewers_receive_independent_worktrees
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake snapshot WorktreeAllocator
  Given execute.dot on the real Engine with a fake allocator returning "/tmp/agentd-task-wt" for implement and distinct reviewer snapshot paths
  When implement succeeds and the review fan-out parks
  Then no reviewer SpawnRequest uses "/tmp/agentd-task-wt"
  And each reviewer SpawnRequest uses a distinct reviewer worktree

Scenario: execute.dot declares the worktree bridge explicitly
  Test: execute_dot_verify_lifecycle_uses_worktree_variable
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/execute.dot and the literal bridge token "${worktree}"
  When it is parsed and validated
  Then the verify_lifecycle node's cmd contains "--code ${worktree}"
