spec: task
name: "Migrate non-execute PR workflows to worktree publication"
tags: [e2e, workflow, p2, worktree, pr]
---

## Intent

P100 made `execute.dot` publish the allocated implementation worktree to a
task-keyed branch before opening a PR. The other shipped PR workflows still use
the old ambient-repo path: `docs-only.dot`, `bugfix-rapid.dot`, and
`refactor-only.dot` open PRs without first publishing `${worktree}`, and the
gated code workflows still verify `"."`. This slice brings those three
workflows onto the same explicit branch publication convention while leaving
non-PR workflows alone.

## Decisions

- `docs-only.dot`, `bugfix-rapid.dot`, and `refactor-only.dot` gain a
  `publish_branch` tool node before `open_pr`.
- `publish_branch` uses the same argv-safe helper as execute:
  `bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}`.
- Each migrated `open_pr` uses `bash scripts/agentd_open_pr.sh ${task_run_id}`.
- `bugfix-rapid.dot` and `refactor-only.dot` verify the allocated worktree with
  `agent-spec lifecycle ... --code ${worktree} ...`.
- The migrated publication edges are success-conditioned:
  `publish_branch -> open_pr` and `open_pr -> report`. `docs-only.dot` also
  gates `write_docs -> publish_branch` on success because it has no goal-gate
  recovery path.

## Boundaries

### Allowed Changes

- workflows/docs-only.dot
- workflows/bugfix-rapid.dot
- workflows/refactor-only.dot
- crates/agentctl/tests/**
- specs/e2e/**
- specs/workflow/p84-builtin-workflows.spec.md

### Forbidden

- Do not modify `execute.dot`; P100 already covers it.
- Do not migrate `spike.dot`, `draft.dot`, or `bootstrap.dot`.
- Do not add new handlers, grammar, or tool-cwd behavior.
- Do not change the publish helper contract from P100.

## Out of Scope

- Real `gh` auth/token provisioning.
- Independent reviewer worktrees.
- Changing the global `goal_gate` recovery model.

## Completion Criteria

Scenario: docs-only publishes its allocated worktree before opening PR
  Test: docs_only_dot_publishes_worktree_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/docs-only.dot
  When it is parsed and validated
  Then publish_branch runs "bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}"
  And open_pr uses "bash scripts/agentd_open_pr.sh ${task_run_id}"
  And write_docs-to-publish, publish-to-open_pr, and open_pr-to-report edges are success-conditioned

Scenario: bugfix-rapid verifies and publishes the allocated worktree
  Test: bugfix_rapid_dot_uses_worktree_and_publishes_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/bugfix-rapid.dot
  When it is parsed and validated
  Then verify_lifecycle uses "--code ${worktree}"
  And publish_branch runs "bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}"
  And open_pr uses "bash scripts/agentd_open_pr.sh ${task_run_id}"

Scenario: refactor-only verifies and publishes the allocated worktree
  Test: refactor_only_dot_uses_worktree_and_publishes_before_pr
  Level: workflow unit
  Test Double: DOT parser + NodeGraph validator
  Given workflows/refactor-only.dot
  When it is parsed and validated
  Then verify_lifecycle uses "--code ${worktree}"
  And publish_branch runs "bash scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}"
  And open_pr uses "bash scripts/agentd_open_pr.sh ${task_run_id}"

Scenario: docs-only publish failure stops before open_pr
  Test: docs_only_dot_publish_failure_stops_before_open_pr
  Level: workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given docs-only.dot on the real Engine and a publish_branch tool result with exit status 1
  When write_docs succeeds
  Then the run fails without recording the open PR helper command

Scenario: migrated PR workflows walk to done with task branch publication
  Test: migrated_pr_workflows_walk_to_done_with_task_branch_publication
  Level: workflow integration
  Test Double: FakeBackend + RecordingCommandRunner + InMemoryStore + fake WorktreeAllocator
  Given docs-only.dot, bugfix-rapid.dot, and refactor-only.dot on the real Engine
  When their agent parks are completed and their gates or reviewers pass
  Then each run reaches Finished after recording publish_branch and `bash scripts/agentd_open_pr.sh ${task_run_id}`
