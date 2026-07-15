spec: task
name: "real execute run-unique evidence"
tags: [smoke, execute, evidence, p153]
---

## Intent

Make the real `execute.dot` smoke prove that the current run produced a
publishable task delta. Replace the reusable static marker contract with a
run-unique contract, reject no-op implementation and publication paths, and
keep all harness state outside tracked runtime files.

## Decisions

- The default input is `specs/e2e/real-execute-smoke-template.spec.md`.
- A smoke run id contains only ASCII letters, digits, `.`, `_`, or `-`.
- The rendered document is `docs/real-execute-smoke/<run-id>.md`.
- The rendered Rust test is `crates/agentd-bin/tests/real_execute_smoke_<rust-id>.rs`, where each non-alphanumeric run-id character becomes `_`.
- The rendered marker is `AGENTD_REAL_EXECUTE_SMOKE_READY:<run-id>`.
- `--prepare-only` renders the spec, plan, workflow, and report paths under the selected `STATE_DIR` without starting the daemon, agents, or GitHub commands.
- The smoke-local workflow compares the implementation worktree with the exact starting `HEAD` before lifecycle review.
- Publication rejects a clean `HEAD` equal to the exact task base before push, while accepting an agent-created commit that descends from and differs from that base.

## Boundaries

### Allowed Changes

- scripts/agentd_real_execute_smoke.sh
- scripts/agentd_verify_task_delta.sh
- scripts/agentd_publish_worktree.sh
- specs/e2e/real-execute-smoke-template.spec.md
- specs/e2e/p153-real-execute-run-unique-evidence.spec.md
- crates/agentd-bin/tests/real_execute_smoke.rs
- crates/agentd-bin/tests/real_execute_task_delta.rs
- crates/agentd-bin/tests/publish_worktree.rs
- docs/superpowers/specs/2026-07-15-real-execute-run-unique-evidence-design.md
- docs/superpowers/plans/2026-07-15-real-execute-run-unique-evidence.md

### Forbidden

- Do not invoke Claude in the real acceptance run.
- Do not change the shipped `workflows/execute.dot` contract.
- Do not populate the deferred `task_runs` provenance columns in this task.
- Do not add a dependency or database migration.
- Do not treat a failed reviewer, delta, publish, or PR gate as success.

## Out of Scope

- AD-E4 execution evidence and certification protocol implementation.
- Runtime worker identity, leases, sandboxing, or scheduler changes.
- Merging smoke-generated artifact pull requests into `main`.

## Completion Criteria

Scenario: dry-run describes a run-unique contract
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_dry_run_prints_run_unique_contract
  Given run id "p153-contract-01"
  When the real execute smoke runs in dry-run mode
  Then stdout names `specs/e2e/real-execute-smoke-template.spec.md`, the unique document, Rust test, test filters, marker, exact starting HEAD, and task delta gate

Scenario: prepare-only isolates generated harness state
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_prepare_only_renders_isolated_contract
  Level: integration test
  Test Double: temporary state directory and local script process
  Given a temporary state directory and run id "p153-prepare-01"
  When the real execute smoke runs with `--prepare-only`
  Then the rendered spec, plan, workflow, and report path are under that state directory
  And tracked `.agentd/run/frozen.spec.md`, `.agentd/run/plan.md`, and `.agentd/run/report.md` are not written

Scenario: invalid run ids fail before creating state
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_rejects_unsafe_run_id_before_state_creation
  Given run id "unsafe/run id"
  When dry-run or prepare-only validation runs
  Then the command exits non-zero and the state directory does not exist

Scenario: unchanged task worktree is rejected
  Test:
    Package: agentd-bin
    Filter: real_execute_task_delta_rejects_unchanged_worktree
  Given `crates/agentd-bin/tests/real_execute_task_delta.rs`, a clean Git worktree, and its exact base commit
  When `agentd_verify_task_delta.sh` runs
  Then it exits non-zero and reports that no task delta exists

Scenario: untracked task output is accepted
  Test:
    Package: agentd-bin
    Filter: real_execute_task_delta_accepts_untracked_change
  Given a clean Git worktree and its exact base commit
  When one untracked task artifact is added and the delta verifier runs
  Then it exits zero

Scenario: committed task output is accepted
  Test:
    Package: agentd-bin
    Filter: real_execute_task_delta_accepts_committed_change
  Given a clean Git worktree and its exact base commit
  When one task artifact is committed and the delta verifier runs
  Then it exits zero

Scenario: invalid task base is rejected
  Test:
    Package: agentd-bin
    Filter: real_execute_task_delta_rejects_invalid_base
  Given a Git worktree and an unavailable base object
  When the delta verifier runs
  Then it exits non-zero before attesting a task delta

Scenario: publication rejects an empty task delta
  Test:
    Package: agentd-bin
    Filter: publish_worktree_rejects_empty_delta_before_push
  Given a valid clean worktree and valid task run id
  When `agentd_publish_worktree.sh` runs
  Then it exits non-zero before commit or push and does not write a published report

Scenario: publication still commits and pushes a real task delta
  Test:
    Package: agentd-bin
    Filter: publish_worktree_writes_local_acceptance_report
  Given a valid worktree with one task change
  When `agentd_publish_worktree.sh` runs
  Then it creates a task commit, pushes `agentd/<task-run-id>`, and writes the selected run-local report

Scenario: Codex-only plan requires run-unique branch and PR evidence
  Test:
    Package: agentd-bin
    Filter: real_execute_smoke_codex_only_success_requires_run_unique_branch_and_pr
  Given runtime matrix `codex,codex,codex,codex` and a fresh run id
  When `crates/agentd-bin/tests/real_execute_smoke.rs` runs the smoke in dry-run mode
  Then stdout requires both run-unique files, `agentd/<task-run-id>`, and a real pull request without selecting a Claude role
