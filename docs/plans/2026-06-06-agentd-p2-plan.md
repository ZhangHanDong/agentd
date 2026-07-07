# agentd — P2 implementation plan (unfreeze core + targeted unblocks)

> Status: as-built plus remaining real-env gates. Reconciles the design-doc
> [§7.3 P2](../specs/2026-05-29-agentd-design.md) ("architectural cleanup") with
> what the P1 season actually proved is needed. **The headline P2 item is already
> done; P2 is smaller and more targeted than the design doc implies.** Successor
> to [the §7.3 P1 roadmap](2026-06-05-agentd-p1-roadmap.md).

## 0. The correction (verified, not assumed)

The design doc frames P2 as *"truly extract agentd-core to hex-core (no sqlx, no
rmcp); store/mempal/backend become adapter crates."* Checked against the code:

- **Dependency extraction is DONE.** `agentd-core/Cargo.toml` deps are only
  tokio/thiserror/serde/serde_json/tracing/async-trait/futures/sha2/ulid — **no
  sqlx, no rmcp.** `agentd-store`/`agentd-tmux`/`agentd-mempal` already exist as
  separate adapter crates.
- **The port boundary is structurally clean too.** The `Store` trait
  (`ports/store.rs`) takes domain types (`RunId`, `NodeId`, `ReviewRunId`,
  `Checkpoint`), not row shapes — no leaked SQL abstractions.
- **The structural items are untouched but OPTIONAL.** No `agentd-domain` /
  `agentd-daemon` / `agentd-cli` crates exist. Splitting them is cosmetic — the
  dep-extraction they were meant to enable is already in place.

So P2 is NOT a 200–400-round extraction refactor. It is: **lift the D1 freeze and
make a few targeted, additive core changes that activate the P1 work three walls
deferred** — with the existing test suite as the regression net that replaces the
freeze. The optional structural splits are weighed in §5, not the spine.

## 0.5 Current as-built status

Worktree activation is delivered in the local runtime path. The daemon injects `WorktreePool`
through `ProductionRunHost::with_worktree_allocator`, and `execute.dot` consumes `${worktree}` for `agent-spec lifecycle --code` and for
`scripts/agentd_publish_worktree.sh ${worktree} ${task_run_id}`. The same
activation line is covered by the follow-on P99-P104/P106/P107 specs: keyed
task-run allocation, branch publication from the implementer worktree, release
after terminal success, reviewer snapshot worktrees, failed-run cleanup, and
maintenance CLI hardening.

The remaining real execute smoke gate is operator-gated environment coverage,
not missing local wiring. A 2026-07-07 real attempt
(`real-execute-smoke-20260707070439`) produced useful partial evidence:
`partial_execute_chain_verified_publish_ok_pr_blocked`. It reached implement,
verify_lifecycle, review, aggregate, and publish_branch, then stopped at
`failed_at_open_pr` because the published branch had no common history with
`origin/main`; Claude also hit a monthly spend limit, so the operator manually
submitted implement and review outcomes through agentd MCP stdio. That leaves
the remaining full real execute smoke gate open: a clean
`AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute`
run where the real agent path and real `open_pr` path complete without manual
substitution or PR-history repair. Non-destructive dry-run/preflight checks are
safe local evidence; the opt-in execute run remains the real-environment
capstone.

## 1. The safety model (what replaces D1)

D1 (never edit `agentd-core/**`) was P0/P1's stability anchor. P2 lifts it; the
replacement guarantee is:

- **84 core unit tests** across 10 files (engine_execute, goal_gate,
  handlers_park, checkpoint, outcome_edge, node_graph, dot_parser, …) plus the
  surface/bin/store integration suites. Every core change is TDD'd against them;
  a regression turns one red.
- **Checkpoint/replay invariants preserved.** Verified: `Checkpoint` is
  `{run_id, current_node, completed_nodes, retry_counts, context_snapshot,
  workflow_sha}` — it does NOT persist `ParkReason` or `RunProgress`. So the
  in-memory park types can gain fields with **no checkpoint migration**; park
  state is reconstructed from the store on resume.
- **Store migrations are additive + defaulted.** The schema changes below land
  via the store's existing migration path; a deployed daemon's existing rows must
  deserialize under the new schema (new columns nullable/defaulted). The store's
  migration tests guard this — and they, not the 84 fresh-state core tests, are
  what catch a back-compat break.

> The one cross-cutting risk: the 84 core tests run on FRESH state, so they stay
> green even if a schema change breaks a *deployed* daemon's existing store. Every
> P2 store change therefore needs an explicit back-compat migration test (old-row
> → new-schema deserialize), not just a fresh-state walk.

## 2. The three unblocks (load-bearing)

### C1 — thread a worktree through `HandlerCtx`  (unblocks P1.3 activation + the bridge)

**Wall:** the frozen `spawn_request` hardcodes `worktree="."`; the frozen tool
handler builds `RunOpts { cwd: None }`. So agents collide in the repo root, and
activating the P1.3 pool would strand the agent's work because tool nodes
(`verify_lifecycle`, `open_pr`, bootstrap's `lint`) run in the daemon cwd, blind
to the agent's worktree (see p6 spec's bridge-scope note — this affects EVERY
tool+agent workflow, not just execute.dot).

**Change:** carry a resolved worktree on `HandlerCtx`; `codergen` builds its
`SpawnRequest` with it (not `"."`), and the `tool` handler sets `RunOpts.cwd` to
it. `RunOpts.cwd` ALREADY exists and `TokioCommandRunner` already honors it — the
tool handler just passes `None` today, so the runner side is a no-op change.
Retires the deferred `PooledBackend` decorator (the per-spawn override + boot-GC
hack) in favor of a worktree threaded from where the run starts.

**Per-run vs per-task_run — VERIFIED (2026-06-07), and it has a prerequisite.**
The entry-gate question (*≤1 open writer task_run per run at a time?*) was checked
against the engine's park/deliver flow:
- The engine is strictly sequential WITHIN a call (`run_loop`/`step_once` runs one
  node, returns at the first `Park`), and `codergen` is the SOLE writer-task_run
  source (one `insert_task_run` per park); `fan_out` makes `review_run`s +
  read-only reviewer agents, never writer task_runs. So per *call*, ≤1 writer
  task_run holds. ✓
- BUT the daemon delivers CONCURRENTLY with no per-run lock, and the shipped
  N-reviewer review is N concurrent `submit_review → deliver_event` on one run.
  Race: all N `lookup_park_by_review_run` (gated `count < expected`) resolve the
  SAME open park before any `insert_review_verdict`; then all N count
  `collected == expected` and all return `Done` → **N concurrent `run_loop`s on one
  run**. Via `goal_gate_unmet → implement`, each spawns a codergen → **multiple
  open writer task_runs on one run** → per-run worktree collides. (It is also a
  pre-existing double-advance / double-`gh pr create` bug — see Risks — and the
  contract test misses it by submitting the 3 verdicts SEQUENTIALLY.)

**Conclusion:** per-run is the right model, but it is safe ONLY WITH **per-run
delivery serialization** in the daemon (serialize `deliver_event` per `run_id` — a
per-run lock/queue/actor). With it, concurrent same-run events queue, the engine
stays sequential, ≤1 writer task_run holds — assumption SATISFIED, not assumed.
That serialization is independently valuable (it closes the latent race below), so
it is promoted to a P2 foundation (§4 step 1).
- *Per-task_run* (the design-doc/P1.3 literal) would avoid the per-run assumption
  but does NOT fix the double-advance race (which exists regardless), and costs the
  spawn→task_run correlation (C3). Not recommended — fix the root (serialization).

**Migration:** worktree is runtime-resolved (like the graph from `workflow_path`)
— NOT checkpoint state. Per-run needs no new persisted column; per-task_run would
persist `worktree_path` on `task_runs` (already a column, currently unused).

**Rounds:** ~60–90 (HandlerCtx field + codergen/tool wiring + run-start allocation
+ release-on-terminal + retire decorator + tests).

### C2 — surface the Delphi round  (unblocks P1.4 Delphi)

**Wall:** `RunProgress::Parked { node_id, reason }` and
`ParkReason::ReviewVerdicts { review_run_id, expected }` carry no round, so the
`emit` point builds `{"node":"review"}` identically every Delphi round → the #6
same-node dedup swallows rounds 2..N.

**Change:** add a round to `review_runs` (store) + `ParkReason::ReviewVerdicts`
(reconstructed from it on resume) + the emit payload (`{"node":"review","round":k}`),
so consecutive same-node re-parks at DIFFERENT rounds are distinct and survive the
dedup. The dedup itself (`event_repo::last` compare) needs no change once the
payload differs by round.

**Migration:** `review_runs` gains a `round` column (default 1) — a store
migration with a back-compat test (existing review_runs read as round 1). NOT a
checkpoint migration (`ParkReason` is not persisted). `ParkReason` gaining a field
is an in-memory change, safe.

**Rounds:** ~50–70, and it is the prerequisite for the Delphi loop itself (the
N-round fan_out/aggregator iteration, the meat of P1.4 — budget that separately,
~140 per the §7.3 estimate).

### C3 — task_run↔worktree correlation  (only if C1 = per-task_run)

Folded into C1. If C1 lands per-run (recommended), C3 disappears. If per-task_run,
C3 = thread the task_run id into the worktree allocation + persist `worktree_path`
+ release on `complete_task_run`. ~40 rounds, conditional.

## 3. Dependent re-activations (after the core changes)

- **Activate P1.3** (post-C1): delete p6's "activation deferred" Out-of-Scope; the
  worktree is now threaded, so isolation is real AND the pipeline sees the agent's
  work. Add an end-to-end walk proving a tool node reads the agent's worktree.
- **P1.4** (post-C2): per-reviewer stance pack (distinct mempal queries / prompt
  profiles — verify whether this needs a `fan_out` handler change or is pure
  config) + the Delphi N-round loop (`visibility=delphi` + `converge_or_*`
  aggregator, design §2.5.1).
- **Per-task_run cleanup** (post-C1): if per-run, this is moot; the run-worktree
  releases at terminal, no boot-GC-only hack.

## 4. Sequencing & rough budget

0. **Foundation A: per-run delivery serialization** (~40) — serialize
   `deliver_event` per `run_id` in the daemon. C1's per-run worktree REQUIRES it
   (verified §2/C1), and it independently closes the latent concurrent-verdict
   double-advance race (§6) that ships TODAY. Must precede C1. Add a CONCURRENT
   N-verdict test (the existing contract test submits sequentially and misses it).
1. **Foundation B: store back-compat migration test harness** (~30) — the net the
   84 fresh-state core tests don't provide; precedes any schema change.
2. **C1 worktree threading** (~60–90) → **activate P1.3** (~30). Highest value:
   unblocks isolation end-to-end for every tool+agent workflow. Depends on Fnd A.
3. **C2 Delphi round** (~50–70) → **P1.4 Delphi loop + stance pack** (~140).
4. **(Optional) structural cleanup** (§5).

Total load-bearing: ~400–470 rounds incl. the re-activations + the serialization
foundation (vs the design doc's 200–400 for "extraction" that's already done — the
budget moved from refactor to feature-unblock + the race fix C1 surfaced).

## 5. Optional structural items (weighed, low priority)

The design doc's other P2 items, reframed now that dep-extraction is done:

- **`agentd-domain` pure-types crate** — pull the `types/` module out of
  `agentd-core` so adapters depend on types without the engine. Cosmetic today
  (core is already dep-clean); justified only if a non-engine consumer needs the
  types. **Defer until there's a second consumer.**
- **Split `agentd-bin` → `agentd-daemon` + `agentd-cli`** — `agentctl` is already
  a separate crate; `agentd-bin` is the daemon + composition root. The split buys
  little. **Defer.**
- **Headless backend (`agentd-headless`)** — design-doc P3, not P2.

Recommendation: do NONE of these in P2 unless a concrete second consumer appears.
They are the part of the design-doc P2 that the verified dep-extraction made
redundant.

## 6. Risks

- **Deployed-store back-compat** (the spine risk): every C2/C3 schema change must
  read existing rows. Mitigated by §4 step 1 (migration-test harness first).
- **Unfreezing core regresses a subtle invariant** the 84 tests don't cover
  (e.g. a park/replay edge). Mitigation: TDD each change; treat any engine_execute
  / checkpoint / goal_gate test flake as a stop-the-line.
- **Concurrent-verdict double-advance race (LATENT, verified 2026-06-07; FIXED by
  Foundation A, committed).** The shared `ProductionRunHost.deliver` had no per-run
  lock; N reviewers can all resolve the same review park before any verdict insert,
  then all see `collected == expected` and all advance the run → double
  `gh pr create` / double-finish / (via the goal-gate loop) multiple writer
  task_runs. NOT worktree-specific — a pre-existing correctness bug. It is LATENT
  (not live) because the rmcp/MCP wire that lets agent processes reach `deliver`
  concurrently is deployment-deferred — but certain once that wire lands (the
  real-agent path). The contract test missed it (verdicts submitted sequentially).
  CLOSED by §4 Foundation A (per-run delivery serialization in the shared host),
  which is also C1's prerequisite — the worktree work and this fix share one root.
- **Scope creep into the optional refactor** (§5). Hold the line: P2 is unblock,
  not rearchitecture — the rearchitecture is already done.
