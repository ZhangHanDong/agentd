# Real Execute Run-Unique Evidence Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make every default real-execute smoke produce and verify a run-specific task delta before review, publication, and PR creation.

**Architecture:** Render a versioned task-spec template into the run evidence directory, inject an exact-base delta node into the smoke-local workflow, and make publication base-aware. The shipped execute workflow and deferred task provenance schema remain unchanged.

**Tech Stack:** Bash, Rust integration tests, Git worktrees, agent-spec, tmux, Codex, GitHub CLI.

## Global Constraints

- Do not invoke Claude in the real acceptance run.
- Do not modify `workflows/execute.dot`.
- Do not add dependencies or migrations.
- Keep generated spec, plan, workflow, report, database, and logs under the selected `STATE_DIR`.
- A failed delta, reviewer, publish, or PR gate remains a failed smoke.

---

### Task 1: Run-Unique Spec Rendering and State Isolation

**Files:**
- Create: `specs/e2e/real-execute-smoke-template.spec.md`
- Modify: `scripts/agentd_real_execute_smoke.sh`
- Modify: `crates/agentd-bin/tests/real_execute_smoke.rs`

**Interfaces:**
- Consumes: CLI `RUN_ID`, `STATE_DIR`, optional `--spec-file`.
- Produces: `FROZEN_SPEC_COPY`, `PLAN_COPY`, `SMOKE_EXECUTE_WORKFLOW`, `REPORT`, run-specific document/test paths, filters, and marker.

- [ ] **Step 1: Write failing dry-run, prepare-only, and unsafe-id tests**

Add tests that invoke:

```text
bash scripts/agentd_real_execute_smoke.sh --dry-run --run-id p153-contract-01
bash scripts/agentd_real_execute_smoke.sh --prepare-only --run-id p153-prepare-01 --state-dir /tmp/agentd-p153-prepare
bash scripts/agentd_real_execute_smoke.sh --prepare-only --run-id 'unsafe/run id' --state-dir /tmp/agentd-p153-unsafe
```

Assert the first command prints:

```text
docs/real-execute-smoke/p153-contract-01.md
crates/agentd-bin/tests/real_execute_smoke_p153_contract_01.rs
AGENTD_REAL_EXECUTE_SMOKE_READY:p153-contract-01
verify_task_delta
```

Assert prepare-only renders all harness files under the Rust test's temporary `state_dir` and unsafe input creates no state directory.

- [ ] **Step 2: Run the new selectors and confirm RED**

```bash
cargo test -p agentd-bin real_execute_smoke_dry_run_prints_run_unique_contract
cargo test -p agentd-bin real_execute_smoke_prepare_only_renders_isolated_contract
cargo test -p agentd-bin real_execute_smoke_rejects_unsafe_run_id_before_state_creation
```

Expected: FAIL because `--prepare-only` and run-unique rendering do not exist.

- [ ] **Step 3: Add the versioned template**

The template uses these literal tokens:

```text
__AGENTD_REAL_EXECUTE_RUN_ID__
__AGENTD_REAL_EXECUTE_RUST_ID__
__AGENTD_REAL_EXECUTE_DOC_PATH__
__AGENTD_REAL_EXECUTE_TEST_PATH__
__AGENTD_REAL_EXECUTE_MARKER__
__AGENTD_REAL_EXECUTE_EXISTS_FILTER__
__AGENTD_REAL_EXECUTE_MARKER_FILTER__
```

Its allowed changes are the rendered document and Rust integration test only. The generated Rust test reads `../../../docs/real-execute-smoke/__AGENTD_REAL_EXECUTE_RUN_ID__.md` and exposes both run-specific filters.

- [ ] **Step 4: Implement validation, rendering, and isolated paths**

In `agentd_real_execute_smoke.sh`:

```bash
validate_run_id() {
    if [[ -z "$RUN_ID" || ! "$RUN_ID" =~ ^[A-Za-z0-9._-]+$ ]]; then
        echo "invalid run id: $RUN_ID" >&2
        return 2
    fi
}

RUST_ID="${RUN_ID//[^[:alnum:]]/_}"
SMOKE_DOC_REL="docs/real-execute-smoke/$RUN_ID.md"
SMOKE_TEST_REL="crates/agentd-bin/tests/real_execute_smoke_${RUST_ID}.rs"
SMOKE_MARKER="AGENTD_REAL_EXECUTE_SMOKE_READY:$RUN_ID"
```

Render to `$STATE_DIR/frozen.spec.md`, generate `$STATE_DIR/plan.md`, place the workflow and report in the same state directory, and add `--prepare-only` without daemon, agent, or GitHub side effects.

- [ ] **Step 5: Run Task 1 tests and contract validation**

```bash
cargo test -p agentd-bin real_execute_smoke
agent-spec parse specs/e2e/real-execute-smoke-template.spec.md
agent-spec lint specs/e2e/real-execute-smoke-template.spec.md --min-score 0.7
```

Expected: all selected tests pass and the rendered-template structure has non-zero scenarios.

- [ ] **Step 6: Commit Task 1**

```bash
git add specs/e2e/real-execute-smoke-template.spec.md scripts/agentd_real_execute_smoke.sh crates/agentd-bin/tests/real_execute_smoke.rs
git commit -m "feat(smoke): render run-unique execute contracts"
```

### Task 2: Exact-Base Task Delta Gate

**Files:**
- Create: `scripts/agentd_verify_task_delta.sh`
- Create: `crates/agentd-bin/tests/real_execute_task_delta.rs`
- Modify: `scripts/agentd_real_execute_smoke.sh`

**Interfaces:**
- Consumes: `agentd_verify_task_delta.sh "$WORKTREE" "$BASE_COMMIT"`.
- Produces: exit 0 only for committed, staged, unstaged, or untracked task output relative to an ancestor base commit.

- [ ] **Step 1: Write failing verifier tests**

Create temporary Git repositories and bind these exact test names:

```text
real_execute_task_delta_rejects_unchanged_worktree
real_execute_task_delta_accepts_untracked_change
real_execute_task_delta_accepts_committed_change
real_execute_task_delta_rejects_invalid_base
```

- [ ] **Step 2: Run verifier tests and confirm RED**

```bash
cargo test -p agentd-bin --test real_execute_task_delta
```

Expected: compile or execution failure because the verifier script is absent.

- [ ] **Step 3: Implement exact-base verification**

The script must validate the Git root and commit object, require the base to be an ancestor of `HEAD`, then pass when either command reports a delta:

```bash
git -C "$worktree" diff --quiet "$base_commit" HEAD --
git -C "$worktree" status --porcelain --untracked-files=all
```

An unchanged worktree prints `no task delta relative to $BASE_COMMIT` to stderr and exits non-zero.

- [ ] **Step 4: Inject the smoke-local gate**

Capture `TASK_BASE_SHA=$(git -C "$ROOT" rev-parse HEAD)` before agents start. In the copied workflow only, insert:

```dot
"verify_task_delta" [handler="tool", cmd="bash scripts/agentd_verify_task_delta.sh ${worktree} $TASK_BASE_SHA", timeout_secs="60"];
"implement" -> "verify_task_delta";
"verify_task_delta" -> "verify_lifecycle";
```

- [ ] **Step 5: Run verifier and workflow tests**

```bash
cargo test -p agentd-bin --test real_execute_task_delta
cargo test -p agentd-bin real_execute_smoke_prepare_only_renders_isolated_contract
```

Expected: all tests pass and the generated workflow references the exact 40-hex task base.

- [ ] **Step 6: Commit Task 2**

```bash
git add scripts/agentd_verify_task_delta.sh crates/agentd-bin/tests/real_execute_task_delta.rs scripts/agentd_real_execute_smoke.sh
git commit -m "feat(smoke): reject execute runs without a task delta"
```

### Task 3: Base-Aware Publication

**Files:**
- Modify: `scripts/agentd_publish_worktree.sh`
- Modify: `crates/agentd-bin/tests/publish_worktree.rs`
- Modify: `scripts/agentd_real_execute_smoke.sh`

**Interfaces:**
- Consumes: `agentd_publish_worktree.sh "$WORKTREE" "$TASK_RUN_ID" "$BASE_COMMIT" "$REPORT_PATH"`.
- Produces: a pushed `agentd/$TASK_RUN_ID` containing a delta from the optional exact base and a report at the selected path.

- [ ] **Step 1: Write failing no-op and run-local report tests**

Add `publish_worktree_rejects_empty_delta_before_push`. Update the successful publication test to pass the seed commit and a temporary report path, then assert the remote branch is ahead of the seed and the default repository report was not written.

- [ ] **Step 2: Run publication tests and confirm RED**

```bash
cargo test -p agentd-bin --test publish_worktree
```

Expected: the no-op publication currently succeeds and the custom report argument is unsupported.

- [ ] **Step 3: Implement base-aware publication**

After `git add -A`:

```bash
if ! git -C "$worktree" diff --cached --quiet; then
    git -C "$worktree" commit -m "agentd $task_run_id" >&2
elif [ -n "$base_commit" ] && git -C "$worktree" diff --quiet "$base_commit" HEAD --; then
    echo "refusing publication: no task delta relative to $base_commit" >&2
    exit 65
fi
```

Validate an optional base as an ancestor commit and write the report only after push succeeds.

- [ ] **Step 4: Pass base and report through the smoke-local workflow**

Render the publish node with the exact base and `$STATE_DIR/report.md`; render `report_acceptance` to read that same path.

- [ ] **Step 5: Run publication and prepare-only tests**

```bash
cargo test -p agentd-bin --test publish_worktree
cargo test -p agentd-bin real_execute_smoke_prepare_only_renders_isolated_contract
```

Expected: all tests pass, no-op push is absent from the bare remote, and report state is isolated.

- [ ] **Step 6: Commit Task 3**

```bash
git add scripts/agentd_publish_worktree.sh crates/agentd-bin/tests/publish_worktree.rs scripts/agentd_real_execute_smoke.sh
git commit -m "fix(publish): require an exact task delta"
```

### Task 4: Contract and Real Acceptance

**Files:**
- Modify: `specs/e2e/p153-real-execute-run-unique-evidence.spec.md`
- Modify: `docs/superpowers/specs/2026-07-15-real-execute-run-unique-evidence-design.md`
- Modify: `docs/superpowers/plans/2026-07-15-real-execute-run-unique-evidence.md`

**Interfaces:**
- Consumes: Tasks 1-3 and runtime matrix `codex,codex,codex,codex`.
- Produces: P153 lifecycle evidence and one finished real smoke with a task branch and PR.

- [ ] **Step 1: Run the complete mechanical gate**

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
agent-spec lifecycle specs/e2e/p153-real-execute-run-unique-evidence.spec.md --code . --format json
git diff --check
```

Expected: all commands exit 0; P153 has 10 passing scenarios and no skipped or uncertain result.

- [ ] **Step 2: Run Codex-only preflight**

```bash
AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex \
  bash scripts/agentd_real_execute_smoke.sh --preflight-only
```

Expected: preflight exits 0 without requiring a Claude executable.

- [ ] **Step 3: Run the authorized real smoke**

```bash
AGENTD_REAL_EXECUTE_SMOKE=1 \
AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex \
  bash scripts/agentd_real_execute_smoke.sh --execute --wait-seconds 900
```

Expected: run status `finished`; three reviewer verdicts are recorded; the task branch contains the two run-specific files; `summary.txt` and `report.md` are under the run state directory; a real PR URL is present in evidence.

- [ ] **Step 4: Record exact evidence and commit**

Update the design and plan with the final run id, task branch, PR URL, test counts, and P153 lifecycle result. Then:

```bash
git add specs/e2e/p153-real-execute-run-unique-evidence.spec.md docs/superpowers/specs/2026-07-15-real-execute-run-unique-evidence-design.md docs/superpowers/plans/2026-07-15-real-execute-run-unique-evidence.md
git commit -m "test(smoke): record run-unique execute acceptance"
```
