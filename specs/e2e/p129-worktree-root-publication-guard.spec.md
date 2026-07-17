spec: task
name: "Worktree root publication guard"
tags: [e2e, daemon, worktree, pr, safety]
---

## Intent

Prevent `publish_branch` from accidentally operating on the daemon repository
when a supplied implementation path is only a directory inside that repository,
not an actual git worktree root. The real execute smoke exposed that `git -C`
can climb to the parent repository when `.git` metadata is missing, so both the
publish helper and the real worktree provider need explicit root validation.

## Decisions

- `scripts/agentd_publish_worktree.sh` rejects a supplied path unless that path
  has `.git` metadata and `git -C <path> rev-parse --show-toplevel` resolves to
  the supplied path itself.
- A rejected publish path exits non-zero before `git switch`, `git add`,
  `git commit`, or `git push` can run.
- `GitWorktreeProvider` validates the path returned by `git worktree add` as an
  actual git worktree root before returning it to the daemon.
- Path comparisons canonicalize symlinked temp directories such as `/tmp` and
  `/private/tmp` before deciding whether the git root equals the supplied path.

## Boundaries

### Allowed Changes

- specs/e2e/p129-worktree-root-publication-guard.spec.md
- scripts/agentd_publish_worktree.sh
- crates/agentd-bin/tests/publish_worktree.rs
- crates/agentd-worktree/src/lib.rs

### Forbidden

- Do not change `execute.dot` topology in this slice.
- Do not add schema columns for branch or worktree metadata.
- Do not make tool nodes run with cwd set to the worktree.
- Do not delete or rewrite existing real-execute evidence artifacts.

## Out of Scope

- Repairing the already-pushed branch created by the earlier smoke attempt.
- Solving GitHub PR creation when local and remote histories have no common
  ancestor.
- Changing Claude account quota or real-agent authentication.

## Completion Criteria

Scenario: publish helper rejects a directory that would climb to the parent repo
  Test:
    Package: agentd-bin
    Filter: publish_worktree_rejects_parent_repo_subdirectory_without_git_metadata
  Level: script integration
  Test Double: temporary git repository
  Given a parent git repository and a nested directory under `.agentd/worktrees`
  And the nested directory has no `.git` metadata
  When `agentd_publish_worktree.sh` receives that nested directory as the worktree
  Then it exits non-zero before staging changes
  And stderr explains that the path is not a git worktree root

Scenario: publish helper still accepts a real git worktree root
  Test:
    Package: agentd-bin
    Filter: publish_worktree_writes_local_acceptance_report
  Level: script integration
  Test Double: temporary git repository and bare origin
  Given a real git repository with `.git` metadata at the supplied path
  When `agentd_publish_worktree.sh` publishes it
  Then stdout is only the task branch name
  And `.agentd/run/report.md` records the task run, branch, and worktree

Scenario: provider validation rejects fake nested worktree paths
  Test:
    Package: agentd-worktree
    Filter: git_worktree_root_validation_rejects_parent_repo_climb
  Level: adapter unit
  Test Double: temporary git repository
  Given a parent git repository and a nested directory without `.git` metadata
  When the provider's worktree-root validation checks the nested directory
  Then validation fails instead of accepting the parent repository root

Scenario: provider returns a real git worktree root after allocation
  Test:
    Package: agentd-worktree
    Filter: git_provider_create_returns_valid_worktree_root
  Level: adapter integration
  Test Double: temporary git repository
  Given `GitWorktreeProvider` with a temporary repository and worktree base
  When it creates `wt-task-tr_0123456789ABCDEFGHJKMNPQRS`
  Then the returned path has `.git` metadata
  And `git rev-parse --show-toplevel` resolves to the returned path
