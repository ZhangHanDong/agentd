# Run-Unique Real Execute Smoke Evidence Design

- Date: 2026-07-15
- Scope: `agentd_real_execute_smoke.sh` default artifact task and publication safety
- Status: approved under the operator's standing authorization to continue the roadmap

## Problem

The default real-execute smoke uses two fixed artifact paths and one reusable
marker. Those files now exist on `origin/main`, so an implementer can submit a
successful no-op outcome and both bound tests still pass. The 2026-07-15
Codex-only run `real-execute-smoke-ad-e0-r2` proved the failure mode: the task
worktree was clean, task provenance columns were empty, two independent
reviewers returned blockers, and the aggregate correctly rejected publication.

The smoke must prove that the current run produced a publishable task delta. A
repository state that was already true before the run is not sufficient.

## Considered Approaches

1. Reuse the fixed files and change their marker on every run. This is small,
   but every smoke edits the same paths and creates avoidable PR conflicts.
2. Keep the fixed files and add only a non-empty-diff shell check. This rejects
   a pure no-op, but the bound tests still attest reusable repository state and
   do not identify the current run.
3. Render run-unique artifact and test paths, then independently require a
   delta from the exact task base. This creates two disposable files per smoke
   branch, but gives the strongest deterministic evidence and isolates runs.

Approach 3 is selected. Empty publication is also rejected as defense in depth.

## Design

### Run-Unique Contract

The default smoke input becomes a versioned template. The harness validates a
run id containing only `A-Z`, `a-z`, `0-9`, `.`, `_`, or `-`, then derives:

- document: `docs/real-execute-smoke/<run-id>.md`;
- test file: `crates/agentd-bin/tests/real_execute_smoke_<rust-id>.rs`;
- test filters containing `<rust-id>`;
- marker: `AGENTD_REAL_EXECUTE_SMOKE_READY:<run-id>`.

`<rust-id>` replaces every non-alphanumeric run-id character with `_`. The
harness rejects a default target that already exists at the task base. Custom
`--spec-file` inputs remain supported, but they still pass through the task
delta gate.

### Exact Task Delta Gate

At smoke startup, the harness records the exact `HEAD` commit from which task
worktrees will be allocated. Its smoke-local `execute.dot` copy inserts
`verify_task_delta` between `implement` and `verify_lifecycle`.

The gate receives the implementation worktree and recorded base commit. It
passes when the worktree contains at least one committed, staged, unstaged, or
untracked change relative to that exact base. It fails for:

- an unchanged worktree;
- an invalid or missing worktree root;
- an invalid or unavailable base commit.

The gate does not infer the base from `origin/main`, because the AD-E0 candidate
already differs from main. It uses the exact captured task base so candidate
history cannot masquerade as task output.

### State Isolation

The rendered spec, generated plan, smoke-local workflow, acceptance report,
database, and logs all live under the run's `STATE_DIR`. The smoke does not
overwrite tracked `.agentd/run/frozen.spec.md` or `.agentd/run/plan.md` and does
not share `.agentd/run/report.md` between runs. Absolute state paths are
inserted into the smoke-local workflow copy; the shipped workflow remains
unchanged.

### Publication Safety

`agentd_publish_worktree.sh` continues validating the worktree root and task
run id before staging. The smoke passes the exact task base and a run-local
report path. The helper commits staged task changes, or accepts an agent-created
commit only when `HEAD` differs from and descends from the exact task base. It
rejects a clean `HEAD == task base` instead of pushing pre-existing state. A
successful report therefore always names a branch containing a task delta.

### Evidence

Dry-run output and the final summary identify the task base, rendered spec,
run-specific paths, marker, and delta gate. Existing daemon, snapshot, event,
and PR evidence remain unchanged. The broader population of
`task_runs.base_commit`, `head_commit`, `diff_sha256`, and
`transcript_sha256` is not added here; that belongs to the AD-E4 execution
evidence protocol and remains visible as separate roadmap work.

## Failure Handling

- Invalid run ids fail before the daemon or agents start.
- Existing default artifact targets fail before the daemon or agents start.
- A no-op implementer outcome fails at `verify_task_delta`, before reviewers.
- A no-op publication fails before branch push.
- Any failed gate remains failure evidence and must not be described as smoke
  success.

## Verification

The implementation is complete only when automated tests prove:

- default dry-run output contains run-unique paths, filters, marker, and base;
- prepare-only output keeps the rendered spec, plan, workflow, and report under
  one run-specific state directory;
- invalid run ids are rejected without creating state;
- exact-base delta verification rejects no-op worktrees;
- exact-base delta verification accepts untracked and committed task changes;
- publication rejects an empty staged delta and still publishes a real delta;
- the rendered task contract parses and lints at or above 0.7;
- the full Codex-only real execute smoke reaches `finished`, pushes its task
  branch, and opens a real PR from the run-specific change.

No Claude runtime is used in the real acceptance run.
