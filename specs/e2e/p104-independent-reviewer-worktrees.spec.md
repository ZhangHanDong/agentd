spec: task
name: "Independent reviewer worktrees"
tags: [e2e, core, store, tmux, workflow, p2, worktree, review]
---

## Intent

P99 intentionally let `fan_out` reviewers share the implementer's allocated
worktree so review and verify looked at the same code tree. That is correct for
visibility, but not for reviewer isolation: concurrent reviewer agents can
modify or dirty the same checkout. P104 gives each reviewer an independent
pool-owned worktree while preserving the same review input by snapshotting the
implementer worktree into each reviewer worktree before spawn.

## Decisions

- Extend the core `WorktreeAllocator` port with a snapshot allocation operation:
  `allocate_snapshot(key, source) -> PathBuf`. `fan_out` uses it only when a
  staged `context["worktree"]` exists and an allocator is injected.
- Reviewer allocation keys are deterministic and scoped to the review run:
  `review-${review_run_id}-${reviewer_id}`. The tmux pool maps these to tight
  pool names `wt-review-rr_<ULID>-<reviewer_id>`.
- Snapshot semantics belong to the allocator implementation. The tmux pool
  creates a reviewer-keyed worktree and mirrors the implementer worktree into it
  while excluding `.git`, so reviewers inspect the same source content without
  sharing the same checkout.
- `fan_out` persists a `(review_run_id, reviewer_id) -> worktree` mapping before
  spawning each reviewer. On the first accepted verdict from that reviewer, it
  releases that reviewer worktree best-effort. Duplicate verdicts remain
  idempotent and must not release twice.
- Without an injected allocator, or without a staged implementer worktree,
  `fan_out` keeps the current fallback: reviewers spawn in the staged worktree
  when present, otherwise `"."`.
- The implementer worktree remains the value in `context["worktree"]`; tool
  nodes and publish nodes continue to verify and publish the implementer's tree,
  not any reviewer snapshot.

## Boundaries

### Allowed Changes

- crates/agentd-core/**
- crates/agentd-store/**
- crates/agentd-tmux/**
- crates/agentctl/tests/**
- crates/agentd-bin/tests/**
- specs/e2e/**
- specs/core/**
- specs/tmux/**

### Forbidden

- Do not make tool nodes run in a worktree cwd.
- Do not replace the implementer `context["worktree"]` with a reviewer path.
- Do not release the implementer worktree from `fan_out`.
- Do not weaken duplicate-reviewer idempotency.
- Do not match loose worktree names such as `wt-review-feature` in boot-GC.

## Out of Scope

- Publishing reviewer branches or surfacing reviewer worktree paths through MCP.
- Durable retry queues for failed reviewer worktree release.
- Restart reuse of in-flight reviewer worktrees.
- Multi-codergen context with more than one implementer worktree.

## Completion Criteria

Scenario: execute.dot reviewers receive independent snapshot worktrees
  Test: execute_dot_reviewers_receive_independent_worktrees
  Level: core workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake snapshot WorktreeAllocator
  Given execute.dot on the real Engine with a fake allocator returning "/tmp/agentd-task-wt" for implement and distinct "/tmp/review-*" paths for reviewers
  When implement succeeds and the review fan-out parks
  Then each reviewer SpawnRequest uses a distinct reviewer worktree
  And no reviewer SpawnRequest uses the implementer worktree
  And the tool context still keeps "${worktree}" as the implementer worktree

Scenario: fan_out without allocator preserves current staged-worktree fallback
  Test: fan_out_without_allocator_uses_staged_worktree
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore
  Given a fan_out node with staged context worktree "/tmp/agentd-task-wt" and no WorktreeAllocator
  When fan_out runs
  Then every reviewer SpawnRequest uses "/tmp/agentd-task-wt"

Scenario: reviewer worktrees release once per distinct reviewer verdict
  Test: fan_out_releases_reviewer_worktree_once_per_distinct_verdict
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore + recording WorktreeAllocator
  Given fan_out allocated one reviewer worktree per reviewer and then receives a duplicate verdict from the same reviewer
  When all distinct reviewers eventually submit
  Then release is called once for each distinct reviewer worktree
  And no duplicate release is recorded for the replayed verdict

Scenario: reviewer worktree mapping is take-once in the store
  Test: reviewer_worktree_mapping_is_take_once
  Level: store integration
  Test Double: real SqliteStore on tempfile
  Given a SqliteStore with a review run and reviewer worktree "/tmp/review-claude-sec"
  When the mapping is taken twice for the same reviewer
  Then the first take returns the path
  And the second take returns None

Scenario: reviewer snapshot allocation failure is loud
  Test: fan_out_reviewer_snapshot_failure_does_not_fall_back_to_shared_worktree
  Level: handler integration
  Test Double: FakeBackend + InMemoryStore + failing WorktreeAllocator
  Given a fan_out node with staged context worktree "/tmp/agentd-task-wt" and an allocator that fails reviewer snapshot allocation
  When fan_out runs
  Then it returns the allocator error
  And no reviewer SpawnRequest is recorded with the shared implementer worktree

Scenario: tmux pool supports tight reviewer-keyed worktrees
  Test: pool_allocates_reviewer_keyed_snapshot_worktree
  Level: adapter unit
  Test Double: in-memory WorktreeProvider + temp source tree
  Given a WorktreePool and reviewer key "review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec"
  When allocate_snapshot creates a reviewer worktree from a source tree containing "src/lib.rs"
  Then the provider creates "wt-review-rr_0123456789ABCDEFGHJKMNPQRS-claude-sec"
  And the reviewer worktree contains the copied "src/lib.rs"

Scenario: boot-GC recognizes reviewer-keyed pool names tightly
  Test: pool_worktrees_keeps_reviewer_keyed_names_preserving_foreign
  Level: adapter unit
  Test Double: porcelain fixture
  Given git worktree porcelain output with a valid reviewer-keyed pool worktree and a human worktree named "wt-review-feature"
  When the pool-worktree filter parses it
  Then only the valid reviewer-keyed pool worktree is returned
