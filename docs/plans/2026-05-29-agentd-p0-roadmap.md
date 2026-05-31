# agentd P0 Implementation Roadmap

> **For agentic workers:** This is a **roadmap**, not a step-by-step plan. It catalogues the ten P0 phases, their dependencies, deliverables, and exit criteria. Detailed step plans live in sibling files (`2026-05-29-agentd-p0.<N>-<name>.md`). Phases P0.0, P0.1, P0.2 are fully planned now (foundational; need to be designed together). Phases P0.3 – P0.9 get their own dedicated plans generated via the `superpowers-writing-plans` skill **at the start of each phase** so earlier-phase learnings can feed back.

**Goal:** Ship the MVP described in [`docs/specs/2026-05-29-agentd-design.md`](../specs/2026-05-29-agentd-design.md) §7.1 — an operator can `/run start <issue>`, five agents walk issue → spec → plan → impl → adversarial review → PR, and `kill -9` of the daemon recovers from checkpoint.

**Architecture:** Single Rust binary daemon + thin clients. Workspace of 9 crates — 7 libraries (agentd-core/tmux/store/mempal/github/matrix/surface) + 2 binaries (`agentd-bin` daemon, `agentctl` CLI). SQLite for own state. Hard MCP-client dependency on mempal. Matrix (matrix-sdk) as primary chat UI. GitHub Issues as the source of truth for work intake.

**Tech Stack:** Rust 2024 · `tokio = "1.49"` · `axum` · `sqlx` (SQLite) · `rmcp` (client + server) · `matrix-sdk` · `octocrab` · `thiserror` · `tracing` · `cargo-nextest` · `agent-spec` CLI for self-applied contracts.

---

## Phase Dependency Graph

```
P0.0 Workspace + CI
  │
  ├─→ P0.1 Core domain + Workflow Engine ────┐
  │     │                                     │
  │     └─→ P0.2 Storage (sqlx + 14 tables) ──┤
  │           │                               │
  │           ├─→ P0.3 TmuxBackend v0  ──────┤
  │           │                               │
  │           ├─→ P0.4 mempal MCP client ────┤
  │           │                               │
  │           ├─→ P0.5 GitHub adapter ───────┤
  │           │                               │
  │           ├─→ P0.6 Matrix adapter  ──────┤
  │           │                               │
  │           └─→ P0.7 HTTP+SSE + MCP server ┤
  │                                           │
  └─→ P0.8 Shipped DOT workflows + skills install
        │
        └─→ P0.9 E2E + disaster recovery (depends on all above)
```

Phases P0.3 / P0.4 / P0.5 / P0.6 / P0.7 are **siblings** once P0.2 is done — they can be implemented in parallel by independent worktrees if you want (`/superpowers:using-git-worktrees`). Recommended serial order if single-stream: 0.3 → 0.4 → 0.5 → 0.6 → 0.7 (backend first to unblock workflow demos; mempal next because review semantics need it; GitHub before Matrix because issues drive runs; HTTP/MCP last because everything else feeds events into it).

> **⚠️ Path B reconciliation (remaining phases P0.5+; supersedes the rows below where they conflict).**
> The [specify-boundary doc](../specs/2026-05-29-agentd-specify-boundary.md) §6 is authoritative. P0.0–P0.4
> were Path-B-neutral and shipped as-is. From P0.5 on, the order/scope change:
> - **P0.5 (GitHub adapter) → moved to P1** (Δ6: GitHub is Specify-owned). The only piece agentd keeps is an
>   `open_pr` (`gh pr create`, provisioned token) **node inside `execute.dot`** — built in the workflow phase,
>   not as a crate phase. Issue-sync / webhook / status-checks → Specify.
> - **Next P0 phase = P0.7** (HTTP+SSE + MCP server): on the standalone-MVP critical path (agents in tmux call
>   the 5 tools), lands the real rmcp transport deferred from P0.4, and depends only on P0.0–P0.4.
> - **Then a workflow-authoring phase** (Δ1): the two standalone DOT graphs `draft.dot` + `execute.dot`,
>   `agentctl run start` trigger, the `open_pr` node, and local-file issue/spec/freeze stubs (§7 standalone).
> - **P0.6 (Matrix) → narrowed + deferrable** (Δ5: dispatch listener + notifier only; slash/MAS authority →
>   Specify). The standalone MVP triggers via CLI, so the full 8-spec P0.6 is not on the MVP path.
> - **P0.9 (E2E + disaster recovery)** stays the capstone (real tmux agents + MCP + the two workflows + kill-9
>   resume). Δ7 (`agentd-specify` client) and Δ8 (Specify semantic events) are **P1**.

---

## Phase Catalogue

| Phase | Title | Specs | Scenarios | Rounds | Plan doc | Status |
|-------|-------|-------|-----------|--------|----------|--------|
| P0.0  | Workspace + CI + agent-spec lifecycle + hello-world      | 3 | 9   | 60   | [`p0.0-workspace-and-ci.md`](./2026-05-29-agentd-p0.0-workspace-and-ci.md)   | **done** (tag v0.0.0-p0.0) |
| P0.1  | Core domain + Workflow Engine + ports/fakes (no I/O)     | 9 | 76  | 240  | [`p0.1-core-and-engine.md`](./2026-05-29-agentd-p0.1-core-and-engine.md)     | **done** (tag v0.0.0-p0.1) |
| P0.2  | Storage layer (sqlx + 16 tables + migrations + repos)    | 5 | 15  | 140  | [`p0.2-storage.md`](./2026-05-29-agentd-p0.2-storage.md)                     | **done** (tag v0.0.0-p0.2) |
| P0.3  | TmuxBackend v0 (FakeRunner-tested)                       | 5 | 43  | 180  | [`p0.3-tmux-backend.md`](./2026-05-29-agentd-p0.3-tmux-backend.md)            | **done** (tag v0.0.0-p0.3) |
| P0.4  | mempal MCP client + outbox drainer + consistency check   | 4 | 22  | 110  | [`p0.4-mempal-client.md`](./2026-05-29-agentd-p0.4-mempal-client.md)          | **done** (tag v0.0.0-p0.4) |
| P0.5  | GitHub adapter (octocrab + webhook + status push)        | 3 | 12  | 90   | _generate via writing-plans_                                                 | **→ P1** (Δ6: GitHub→Specify; only `open_pr` node survives) |
| P0.6  | Matrix adapter + slash router + wait.human + threads     | 8 | 28  | 220  | _generate via writing-plans (split into 6a + 6b)_                            | deferred — **narrowed** (Δ5: dispatch listener + notifier) |
| P0.7  | HTTP+SSE + MCP server (5 tools per §4.12.1)              | 5 | 18  | 140  | _generate via writing-plans_                                                 | deferred |
| P0.8  | Shipped DOT workflows + `agentctl install-skills`        | 3 | 10  | 80   | _generate via writing-plans_                                                 | deferred |
| P0.9  | E2E + disaster recovery drills                           | 5 | 18  | 160  | _generate via writing-plans_                                                 | deferred |
| **Σ** |                                                          | **52** | **257** | **1420** | | |

> Scenario counts are derived from each phase's plan-detail file when one exists,
> otherwise from the phase-outline below. The total drifted upward from the
> design doc's §7.2 estimate (187 scenarios) because P0.0/P0.1/P0.2 plans enumerate
> finer test selectors than the design rough-cut did. **The rounds column sums to
> 1420, not the ≈1320 headline the design doc §7.2 carried — that headline was an
> arithmetic slip; the per-phase components (60+240+140+180+110+90+220+140+80+160)
> have always summed to 1420.** Treat 1420 as the working P0 budget.

---

## Cross-Cutting Conventions

These apply to every phase. Establishing them once here keeps phase plans short.

### Repo + Branch

- Single repo: `~/Work/Projects/AI/agentd`, default branch `main`.
- One worktree per phase under `~/Work/Projects/AI/agentd-worktrees/p0.<N>/` (created on demand via `/superpowers:using-git-worktrees`).
- Each task ends with a focused commit; phases close with a PR back to `main`.

### Commit Conventions

- Conventional commits: `feat:` / `fix:` / `docs:` / `test:` / `refactor:` / `chore:`.
- Scope is the crate or area: `feat(core): ...`, `test(tmux): ...`, `docs(plan): ...`.
- Bodies cite the spec file when relevant: `Refs: specs/tmux/p11-prompt-injection-buffer-path.spec.md`.
- Co-author trailer when working in a multi-agent room: `Co-Authored-By: <agent_id> <agentd@noreply>` plus `Workflow-Run: <run_id>` trailer (see design §3.x commit guidance).

### Quality Gate (every PR must pass)

```bash
./scripts/check.sh
```

…which runs (defined in P0.0):

1. `cargo fmt --all --check`
2. `cargo clippy --workspace --all-targets -- -D warnings`
3. `cargo nextest run --workspace`
4. `agent-spec lifecycle specs/**/*.spec --code . --min-score 0.7 --format prompt-summary`
5. `target/debug/agentctl flow validate workflows/*.dot` (once `agentctl` exists; gated in earlier phases)

PRs blocked unless this script exits 0.

### TDD Discipline

For every new behavior:

1. Write the failing test (cite spec scenario name where applicable).
2. Run the test → confirm failure with the expected message.
3. Implement the minimal code.
4. Run the test → confirm pass.
5. `cargo clippy` clean.
6. Commit.

Refactors get the same loop but step 1 is "ensure existing tests still pass before refactor; add characterization test if not."

### Spec Linkage

- Each task should have a `**Spec:**` line pointing to the `.spec.md` file whose scenario it satisfies.
- Test function names match the spec's `Test:` selector verbatim.
- `agent-spec guard` will catch dangling selectors in CI.

### Forbidden Patterns (enforced via clippy / rg / CI)

- `unwrap()` outside tests
- `send-keys -l <payload>` anywhere
- `rg 'palace.db' crates/` returning matches
- `tokio::time::sleep` for test timing (use `tokio::time::pause` + `advance`)
- Real tmux / Matrix / mempal in `--lib` or `--test '*unit*'` / `'*integration*'` test files
- `#[ignore]` without a `TODO #N` comment and ≤ 30-day expiry

---

## Per-Phase Outline (P0.3 – P0.9)

For each deferred phase, this section captures the **deliverable inventory** so that when you call `writing-plans` for that phase you have:

- the list of files to create / modify
- the list of specs to author
- the test selectors expected
- exit criteria

> **Boundary deltas to fold in when planning these phases** (see [`docs/specs/2026-05-29-agentd-specify-boundary.md`](../specs/2026-05-29-agentd-specify-boundary.md) §6). The outlines below predate Path B; when you `writing-plans` a phase, apply its delta:
> - **P0.5 GitHub** — Δ6: GitHub is owned by **Specify**, not agentd. P0.5 shrinks to "execution-time PR open via a Specify-provisioned token" (no issue webhook, no issue mirroring, no status-check ownership). Most of the original P0.5 surface moves to Specify; consider folding the remnant into P0.8 or a thin `agentd-github` PR-only helper.
> - **P0.6 Matrix** — Δ5: agentd is a **dispatch listener + execution notifier**, NOT the Matrix Application Service. The slash-command authority (`/start`, `/execute`, `/spec-approve`) and the canonical state machine live in **Specify**. P0.6 narrows to: connect to the Matrix server via bridge, receive work-token/DAG dispatch, post execution notifications, relay `agent.blocked` decisions. Drop the spec-approval slash router.
> - **P0.8 Shipped DOT** — Δ1: ship **`draft.dot`** (fetch issue from Specify → propose_spec → lint → push draft to Specify) and **`execute.dot`** (pull frozen spec from Specify → plan → impl → verify → adversarial review → PR → report) instead of one `issue-to-pr.dot`. Human review + freeze happen in Specify *between* the two.
> - **New P1 phase — `agentd-specify` client** — Δ7: thin outbound client (pull issue / push draft / pull frozen spec / report semantic events). Not in P0; agentd runs standalone in P0 (boundary §7).

### P0.3 — TmuxBackend v0

- **Crate**: `crates/agentd-tmux/`
- **New files**: `src/lib.rs`, `src/backend.rs`, `src/command_runner.rs`, `src/discover.rs`, `src/launcher.rs`, `src/inject.rs`, `src/probe.rs`, `src/error.rs`, `src/test_support.rs` (feature-gated), `tests/tmux_backend.rs`
- **Specs to author** (under `specs/tmux/`):
  - p10-spawn.spec.md
  - p11-prompt-injection-buffer-path.spec.md
  - p12-status-probe.spec.md
  - p13-shutdown-capture-first.spec.md
  - p14-rebind.spec.md
  - p15-launcher-script.spec.md
  - p16-systemd-strategy.spec.md
- **Test selectors** (must exist as `#[tokio::test]` fns):
  - `test_spawn_creates_session_and_returns_pane_id`
  - `test_spawn_rejects_existing_session_as_recoverable`
  - `test_prompt_injection_uses_buffer_path_not_send_keys_literal`
  - `test_large_prompt_streams_through_stdin_not_argv`
  - `test_missing_session_returns_recoverable`
  - `test_missing_tmux_binary_is_fatal_with_install_hint`
  - `test_shutdown_archives_capture_before_kill`
- **Deps**: `agentd-core` (trait), `tokio`, `thiserror`, `which`, `tempfile` (dev)
- **Depends on**: P0.0, P0.1 (trait definitions)
- **Exit**: all 7 tests pass against `FakeRunner`; no real tmux needed for any test; design §4 fully implemented.

### P0.4 — Mempal MCP Client

- **Crate**: `crates/agentd-mempal/`
- **New files**: `src/lib.rs`, `src/client.rs`, `src/outbox.rs`, `src/drainer.rs`, `src/consistency.rs`, `src/error.rs`, `src/test_support.rs` (`MempalStub`), `tests/client.rs`
- **Specs**: `specs/mempal/p30-outbox-drainer.spec.md`, `p31-consistency-check.spec.md`, `p32-client-mcp-tools.spec.md`, `p33-pre-tools-best-effort.spec.md`
- **Tests**:
  - `test_ingest_via_outbox_does_not_block_workflow`
  - `test_drainer_retries_with_backoff_until_attempts_exceeded`
  - `test_kg_add_writes_outbox_row_in_same_tx_as_node_outcome`
  - `test_pre_tools_search_falls_back_to_empty_on_timeout`
  - `test_consistency_check_reports_missing_drawers`
- **Depends on**: P0.0, P0.2 (`mempal_outbox` table)
- **Exit**: outbox FIFO + backoff verified; mempal-down does NOT stall workflow.

### P0.5 — GitHub Adapter

- **Crate**: `crates/agentd-github/`
- **New files**: `src/lib.rs`, `src/issue_sync.rs`, `src/webhook.rs`, `src/status_push.rs`, `src/error.rs`, `tests/sync.rs`
- **Specs**: `specs/github/p40-issue-mirror-sync.spec.md`, `p41-webhook-signature.spec.md`, `p42-status-push.spec.md`
- **Tests**:
  - `test_issue_pull_inserts_row_with_workflow_dot_resolved`
  - `test_webhook_signature_mismatch_returns_401`
  - `test_status_push_marks_check_run_success_on_pr_open`
- **Depends on**: P0.0, P0.2 (`issues` table)
- **Exit**: webhook end-to-end + polling fallback verified with `wiremock`.

### P0.6 — Matrix Adapter (recommended split into 6a + 6b)

- **Crate**: `crates/agentd-matrix/`
- **6a — slash router + wait.human delivery** (120 rounds):
  - New: `src/lib.rs`, `src/provision.rs`, `src/slash.rs`, `src/permissions.rs`, `src/wait_human.rs`, `src/test_support.rs`
  - Specs: `p20-slash-router-permissions.spec.md`, `p21-wait-human-delivery.spec.md`, `p22-mxid-provisioning.spec.md`, `p23-agent-self-approval-rejected.spec.md`
- **6b — render + thread + cowork-bus gateway** (100 rounds):
  - New: `src/render.rs`, `src/thread.rs`, `src/gateway.rs`
  - Specs: `p24-review-thread-rendering.spec.md`, `p25-cowork-bus-gateway-rules.spec.md`, `p26-event-noise-reduction.spec.md`, `p27-trust-modes.spec.md`
- **Tests** (`matrix-sdk::MockServer`):
  - `test_bind_command_updates_db_and_invites_agents`
  - `test_spec_approve_by_operator_answers_human_wait`
  - `test_spec_approve_by_agent_mxid_is_rejected`
  - `test_audit_mode_allows_non_operator_with_warning`
  - `test_at_mention_forwards_to_cowork_bus_and_adds_eye_reaction`
  - `test_review_fan_out_opens_main_room_card_and_thread`
  - `test_wait_human_card_contains_correct_slash_command_suggestions`
- **Depends on**: P0.0, P0.2 (`matrix_events`, `human_waits`, `projects.matrix_room_id`), P0.4 (mempal client for `mempal_cowork_push`)
- **Exit**: all 7 tests via MockServer; no real homeserver needed.

### P0.7 — HTTP+SSE + MCP Server

- **Crate**: `crates/agentd-surface/`
- **New files**: `src/lib.rs`, `src/http.rs` (axum router), `src/sse.rs`, `src/mcp_server.rs` (rmcp server), `src/tools/{assign_task,submit_review,check_inbox,submit_outcome,query_run}.rs`, `tests/http.rs`, `tests/mcp.rs`
- **Specs**: `specs/surface/p70-mcp-tool-schemas.spec.md`, `p71-sse-event-replay.spec.md`, `p72-submit-outcome-idempotency.spec.md`, `p73-http-routes.spec.md`, `p74-mcp-tool-error-codes.spec.md`
- **Tests**:
  - `test_assign_task_returns_pending_when_no_match`
  - `test_submit_review_is_idempotent_for_same_payload`
  - `test_submit_review_errors_on_conflicting_payload`
  - `test_check_inbox_drains_when_drain_flag_set`
  - `test_submit_outcome_rejects_stale_attempt`
  - `test_query_run_returns_404_for_unknown_run`
  - `test_sse_replay_resumes_from_last_seq`
- **Depends on**: P0.0, P0.1 (engine), P0.2 (store), P0.4 (mempal — `check_inbox` consults cowork-bus)
- **Exit**: tools defined per §4.12.1; SSE replay tested; rmcp server starts cleanly.

### P0.8 — Shipped DOT + Skills Install

- **No new crate**; modify `agentctl` and ship files under `workflows/`
- **New files**: `workflows/issue-to-pr.dot`, `workflows/spec-only.dot`, `workflows/adversarial-review.dot`
- **agentctl changes**: `install-skills` subcommand (idempotent; manages `~/.claude/skills/agent-spec-*` symlinks and verifies `mempal` binary on PATH)
- **Specs**: `specs/workflow/p50-issue-to-pr-canonical.spec.md`, `p51-adversarial-review-frozen-bundle.spec.md`, `p52-install-skills-idempotency.spec.md`
- **Tests**:
  - `test_issue_to_pr_dot_validates`
  - `test_adversarial_review_dot_validates`
  - `test_install_skills_creates_symlinks_first_run`
  - `test_install_skills_is_idempotent_second_run`
- **Depends on**: P0.0 (`agentctl flow validate`), all DOT-handler implementations from P0.1–P0.7
- **Exit**: all three DOTs pass `flow validate`; install-skills idempotent.

### P0.9 — E2E + Disaster Recovery

- **No new crate**; new `e2e/` dir at repo root
- **New files**: `e2e/Dockerfile.synapse`, `e2e/docker-compose.yml`, `e2e/scripts/run.sh`, `e2e/scenarios/*.rs`, `e2e/fixtures/`
- **Specs**: `specs/e2e/p90-happy-path.spec.md`, `p91-mempal-offline-degrade.spec.md`, `p92-kill-9-resume.spec.md`, `p93-tmux-version-matrix.spec.md`, `p94-webhook-replay-protection.spec.md`
- **Tests** (nightly only):
  - `e2e_happy_path_issue_to_pr`
  - `e2e_mempal_offline_workflow_progresses_post_action_queues`
  - `e2e_kill_dash_nine_resumes_from_checkpoint`
  - `e2e_three_reviewer_blind_review_aggregates`
  - `e2e_workflow_sha_change_requires_accept_flag`
- **Depends on**: all prior phases
- **Exit**: nightly e2e 5/5 green; the 8-step demo from §7.8 runs from a clean install in under 90 seconds.

---

## How to Use This Roadmap

1. **Now**: review and approve P0.0, P0.1, P0.2 detailed plans (sibling files).
2. **Per phase start**: when ready to begin P0.3 (or any deferred phase), invoke `superpowers-writing-plans` with the phase's section in this roadmap as input — it will produce the detailed task breakdown.
3. **Per phase end**: tick the row in the catalogue table from "deferred" → "in-progress" → "done"; update spec/scenario counts if reality diverges.
4. **Replan trigger**: if any phase's actual rounds > 200 % of estimate, hold a retrospective and re-estimate the remaining phases.

---

## Open Decisions Deferred to Phase-Start Replanning

- **P0.4 vs P0.6 ordering**: roadmap recommends mempal before matrix because matrix adapter calls `mempal_cowork_push`. If you decide to stub the mempal client during P0.6 development (parallel worktrees), document the stub commitment in P0.6's plan.
- **P0.6a / P0.6b split**: roadmap shows split; if the team decides to ship 6a as a usable milestone (Matrix wait.human only, no review threading yet), 6a's PR can merge to main before 6b starts.
- **e2e Matrix homeserver**: synapse-in-docker is the default. If you prefer conduwuit by phase 0.9 time, change `Dockerfile.synapse` and add a `--homeserver` flag to `e2e/run.sh`.
