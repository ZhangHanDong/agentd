spec: task
name: "Open PR history recovery guidance"
tags: [e2e, workflow, pr, git, recovery, safety]
---

## Intent

Make the `open_pr` no-common-history failure actionable for the exact published
task branch that just failed. P148 showed that the execute chain can reach
`publish_branch` and then stop at `open_pr`; this slice keeps the refusal before
`gh`, but prints the deterministic operator sequence needed to repair and retry
the published `agentd/<task_run_id>` branch.

## Decisions

- `scripts/agentd_open_pr.sh` must still reject a task branch with no merge-base
  with `origin/<base_branch>` before invoking `gh`.
- The no-common-history stderr must include a recovery sequence for the concrete
  `agentd/<task_run_id>` branch: switch to the branch, review
  `agentd_pr_history_bridge.sh` in dry-run mode, opt in with
  `AGENTD_PR_HISTORY_BRIDGE=1 ... --execute`, push the repaired branch to
  `origin`, and retry `agentd_open_pr.sh`.
- The recovery guidance must preserve the requested base branch, including
  non-default bases.
- The helper must not auto-merge unrelated histories, auto-push, rewrite
  history, or invoke `gh` after a failed preflight.
- The P0.9 deployment checklist should mention that post-`publish_branch`
  no-history failures now print task-branch repair guidance, while keeping the
  full real execute gate open.

## Boundaries

### Allowed Changes

- **/specs/e2e/p149-open-pr-history-recovery.spec.md
- **/scripts/agentd_open_pr.sh
- **/crates/agentd-bin/tests/open_pr.rs
- **/crates/agentd-bin/tests/deployment_checklist.rs
- **/docs/p0.9-deployment-checklist.md

### Forbidden

- Do not run `AGENTD_REAL_EXECUTE_SMOKE=1 --execute`.
- Do not run `AGENTD_PR_HISTORY_BRIDGE=1 ... --execute` against this repository.
- Do not change publish_branch semantics.
- Do not change `scripts/agentd_pr_history_bridge.sh` execution semantics.
- Do not push, force-push, or rewrite real local or remote history.
- Do not claim the full real execute smoke completed.

## Out of Scope

- Solving Claude account quota or authenticated agent spend limits.
- Retrying GitHub PR creation against the real P148 run.
- Adding automatic PR-history repair to the workflow.
- Changing workflow base-branch policy.

## Completion Criteria

<!-- lint-ack: error-path - the two open_pr scenarios are failure-path scenarios: they assert non-zero no-common-history preflight exits. -->
<!-- lint-ack: boundary-entry-point - deployment_checklist.rs is bound through the checklist source-inspection scenario's structured Test selector. -->

Rule: open-pr-history-recovery

Scenario: open_pr failure prints task-branch repair guidance on no common history
  Test:
    Package: agentd-bin
    Filter: open_pr_rejects_no_common_history_before_gh
  Level: script integration
  Test Double: temporary git repositories and fake gh
  Given `origin/main` and `agentd/tr_0123456789ABCDEFGHJKMNPQRS` point to unrelated histories
  When `agentd_open_pr.sh` runs for that task id
  Then it exits non-zero before calling `gh`
  And stderr names the missing common history between the task branch and `origin/main`
  And stderr includes commands to switch to the task branch, dry-run the bridge,
  opt in to `AGENTD_PR_HISTORY_BRIDGE=1 ... --execute main`, push the repaired
  task branch, and retry `agentd_open_pr.sh`

Scenario: open_pr no-common-history failure preserves a non-default base branch
  Test:
    Package: agentd-bin
    Filter: open_pr_no_common_history_guidance_uses_requested_base_branch
  Level: script integration
  Test Double: temporary git repositories and fake gh
  Given `origin/release` and `agentd/tr_0123456789ABCDEFGHJKMNPQRS` point to unrelated histories
  When `agentd_open_pr.sh` runs for that task id with base branch `release`
  Then it exits non-zero before calling `gh`
  And stderr names `origin/release`
  And the dry-run, execute, and retry commands use `release` rather than `main`

Scenario: deployment_checklist.rs points operators to post-publish repair guidance
  Test:
    Package: agentd-bin
    Filter: deployment_checklist_mentions_open_pr_history_recovery_guidance
  Level: source inspection
  Given `crates/agentd-bin/tests/deployment_checklist.rs` reads the P0.9 deployment checklist
  When the source-inspection test reads the execute.dot section
  Then it says `agentd_open_pr.sh` reports task-branch repair guidance after
  post-`publish_branch` no-common-history failures
