# agentd — P1 roadmap plan (§7.3 reconciled with the Specify boundary)

> Status: as-built plus remaining gates. This began as a planning note and now
> records the delivered P1 slices as well as the remaining external Specify API
> gate. Reconciles design-doc
> [§7.3](../specs/2026-05-29-agentd-design.md) (the "make v1 nice to use" packs,
> written pre-split) with the authoritative
> [boundary doc](../specs/2026-05-29-agentd-specify-boundary.md). **Where they
> conflict, the boundary doc wins.** This plan filters §7.3 by what agentd
> actually owns, records the fork that drove the early P1 ordering, and keeps the
> current "what remains" boundary explicit.
>
> Naming: the design doc's §7.3 packs are **P1.1–P1.7**. They are NOT the
> ad-hoc "herdr-borrow P1 batch" (#1–#6) already shipped this cycle. Distinct
> numbering; don't conflate.

## 0. Standalone reality check (verified, not assumed)

The shipped `draft.dot` / `execute.dot` "Specify tool nodes" are **local
file/CLI shell-outs**, not Specify API calls:

| Node | `cmd=` | Reality |
|---|---|---|
| `fetch_issue_context` | `cat .agentd/run/issue.md` | local file (the solo stand-in) |
| `pull_frozen_spec` | `cat .agentd/run/frozen.spec.md` | local file |
| `push_draft` | `cp …draft.spec.md …draft.out.spec.md` | local copy |
| `report_acceptance` | `cat .agentd/run/report.md` | local file |
| `open_pr` | `gh pr create --fill` | **real**, ambient `gh` auth |
| `draft_plan` / `lint_spec` / `verify_lifecycle` | `agent-spec …` | **real** CLI |
| `implement` / `review` | codergen / fan_out | **real** agents |

So agentd **already does real, useful work standalone** — it drafts specs, runs
adversarial review, opens PRs — using the `.agentd/run/` file convention as the
Specify stand-in (precisely the self-host mode boundary §7 mandates). The
contract/`#6` tests that drive `execute.dot` to `finished` do so under
`RecordingCommandRunner` (a fake), so they prove the engine completes the graph
on tool success — NOT that the tool nodes do real work. The table above is the
real-work evidence; the tests are not.

**Consequence:** the Specify client (Δ7) is a pure **integration-add** — swap a
local-file stand-in for a real Specify pull — **not** a prerequisite for
standalone value. The fork below is genuinely open.

## 0.5 As-built status after P145

Track A is substantially delivered in this repo:

- **P1.3 Worktree pool** — delivered across P99-P107: per-`task_run`
  allocation, publication, reviewer isolation, boot-GC preservation, failed-run
  cleanup, and maintenance CLI hardening.
- **P1.7 More workflows** — delivered by the shipped `bugfix-rapid`,
  `docs-only`, `refactor-only`, `spike`, and `bootstrap` workflows, with PR
  publication moved onto the worktree branch convention.
- **P1.4 Reviewer stance + Delphi** — delivered through P108-P113: round-aware
  review parks, converge-or fallback, round-loop runtime, reviewer stance packs,
  prompt profiles, and findings-diff convergence.
- **P1.5 agent-spec discover** — delivered by `bootstrap.dot` using
  `agent-spec discover --from-codebase`.
- **P1.1 Dashboard** — delivered by P114-P118 as a read-mostly local operator
  shell over `GET /runs`, `GET /runs/:id`, and the SSE event tail.

Specify Track B is delivered through the local no-network seam and runtime hook:

- **P142** adds the private `agentd-specify` crate, object-safe
  `SpecifyClient`, `OfflineSpecify`, protocol value types, and recording test
  support.
- **P143** adds the first pure semantic-event mapping:
  `run_parked` -> `agent.blocked`, `run_finished` -> `workflow.finished`, and
  `run_failed` -> `workflow.failed`.
- **P144** adds `report_agentd_event`, connecting the mapper to
  `SpecifyClient::report_event` without runtime or network wiring.
- **P145** adds the runtime `ProductionRunHost` hook that reports newly appended
  durable events through the injectable client after live broadcast, defaulting
  to `OfflineSpecify` and treating report failures as best-effort.

What remains is deliberately external-contract gated: **real HTTP/WS transport**,
**auth**, **endpoint config**, and **canonical external workflow ids** require a
concrete Specify API contract. Do not infer or invent those endpoint semantics
from this repo alone.

## 1. Boundary-filtered pack inventory

| Pack | Source | Owner verdict | One-line scope (agentd's part) |
|---|---|---|---|
| **P1.3 Worktree pool** | §7.3 | ✅ agentd | concurrent `task_run` isolation: explicit lock, cleanup policy |
| **P1.7 More workflows** | §7.3 | ✅ agentd | `bugfix-rapid` / `docs-only` / `refactor-only` / `spike` `.dot` |
| **P1.4 Reviewer stance + Delphi** | §7.3 | ✅ agentd (core) | per-reviewer mempal queries; prompt profiles; Delphi N-round loop |
| **P1.5 agent-spec discover** | §7.3 | ✅ agentd | `discover --from-codebase` as a bootstrap workflow |
| **P1.1 Dashboard** | §7.3 | ✅ agentd, **scope-narrowed** | LOCAL operator view (read-mostly); humans use Robrix/Specify, so "write mode" largely overlaps Specify's command authority — trim to a local-ops console |
| **Δ7 Specify client** (`agentd-specify`) | boundary Δ7 | ✅ agentd, **partly delivered / external-contract risk remains** | as-built through P145: `SpecifyClient`, `OfflineSpecify`, `report_agentd_event`, and a runtime hook; real HTTP/WS transport waits on a concrete Specify API contract |
| **Δ8 Semantic-event mapping** | boundary Δ8 | ✅ agentd, **first mapping delivered** | P143 maps current durable run events to Specify semantic events; richer facts wait until runtime emits them explicitly |
| ~~P1.2 wait.human multi-channel~~ | §7.3 | ⚠️ **mostly Specify** | spec-approval wait removed (Δ2); only mid-execution `request_human_decision` stays; the multi-channel relay is Specify's. agentd keeps only the one in-loop park it already has |
| ~~P1.6 Webhook hardening~~ | §7.3 | ❌ **Specify** | GitHub (issues/repos/PR-status/webhooks) owned by Specify (Δ6); inbound webhooks are not agentd's |

## 2. The fork the user must decide

Two coherent near-term goals; they pick different lead packs. The decision input
is a **certainty/risk asymmetry**, not "which is the linchpin":

- **Track A — standalone hardening** (P1.3 → P1.7 → P1.4 → P1.5, optional P1.1):
  high-certainty value, **zero external dependency**. Every pack is pure agentd
  runtime; nothing waits on an API outside this repo.
- **Track B — Specify integration** (Δ7 + Δ8): unlocks the cloud layer, but
  real network transport builds against Specify's HTTP/WS API, which §6 says is
  a **separate repo, out of scope here, and not yet defined**. As of P145 this
  repo has the no-network seam, semantic-event mapping, reporting helper, and
  runtime event hook. The next real integration step is transport/config, and it
  stays gated on an external contract this repo cannot verify by itself.

That asymmetry is the heart of the decision: Track A is shippable certainty;
Track B is partly an IOU against an undefined external surface.

## 3. Recommended sequencing

**Historical recommendation:** lead with Track A, then defer real Track B network
transport until the Specify API contract exists.

1. **P1.3 Worktree pool** (~120) — most load-bearing correctness: without
   per-`task_run` isolation, concurrent runs collide. Foundational, unblocks
   everything concurrent.
2. **P1.7 More workflows** (~120) — high utility, cleanest boundary, low risk;
   exercises the engine on new shapes and surfaces gaps cheaply.
3. **P1.4 Reviewer stance + Delphi** (~140) — deepens agentd's core
   differentiator (adversarial review). **Carries a landmine — see §4.**
4. **P1.5 agent-spec discover** (~60) — small tooling win; bootstrap workflow.
5. **P1.1 Dashboard** (~120 trimmed, was ~200) — only after the above; build as
   a local-ops console, not a second human UI.

**Current Track B boundary:** P142-P145 completed the local in-process seam.
Further Track B work must name the concrete Specify API contract it implements.
Until then, real HTTP/WS transport, auth, endpoint config, and canonical external
workflow ids remain future work.

Rationale: Track A is all high-certainty, zero-external-dep value, and P1.3 is
genuinely foundational (concurrency correctness gates everything else). Track B's
value is real but its *risk* is external and its *prerequisite* (a defined
Specify API) is out of this repo — leading with it would block agentd progress on
something it can't control.

## 4. Cross-cuts & landmines (reconciliation with shipped work)

- **P1.4 Delphi × the #6 re-park dedup (must-fix in P1.4).** #6 (`6f41188`)
  suppresses consecutive `run_parked` with an identical payload. Delphi runs N
  rounds all parked at `review` → identical `{"node":"review"}` → **rounds 2..N's
  park events would be silently swallowed**. P1.4 MUST carry a round
  discriminator in the park payload (e.g. `{"node":"review","round":k}`) or make
  the dedup round-aware. Recorded here so the dedup decision's downstream cost is
  explicit, not rediscovered.
- **Δ7 `OfflineSpecify` seam (non-negotiable).** Boundary §7: agentd must run
  without Specify. The client lands behind a seam exactly like `OfflineMempal`,
  so the `.agentd/run/` file convention remains the standalone path.
- **P1.1 Dashboard scope.** Humans act through Robrix/Specify (Δ5: agentd is a
  dispatch listener/notifier, not the command authority). The dashboard is a
  LOCAL operator/debug console — read-mostly over the control plane
  (`GET /runs`, `GET /runs/:id`, the SSE tail). "Write mode" beyond local ops
  overlaps Specify's authority — keep it thin.

## 5. Resolved fork and remaining gate

The early fork is resolved in practice: agentd first hardened standalone P1
runtime behavior, then advanced Specify Track B through P145 without taking a
network dependency. The remaining gate is no longer "should we create the seam?"
but "which concrete Specify API contract should the real transport implement?"
Until that contract exists, the correct next state is to preserve the
`OfflineSpecify` default, the local `.agentd/run/` convention, and the
best-effort runtime reporting hook.
