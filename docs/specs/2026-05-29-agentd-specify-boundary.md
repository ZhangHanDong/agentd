# agentd ↔ Specify — Boundary & Integration Spec

- **Date**: 2026-05-29
- **Status**: Decided (Path B), amended by P264 — supersedes the implicit "agentd owns everything" framing in the P0 design doc where they conflict
- **Enterprise ownership amendment**: `2026-07-10-enterprise-execution-ownership-boundary.md`
- **Decision**: agentd is the **local execution/orchestration runtime** (the "specify orchestration cli"). **Specify** is a **separate web project** — the cloud collaboration + project-context layer that sits ON TOP of agentd. They connect over an open protocol seam. Specify does NOT duplicate agentd's engine; agentd does NOT own project-context/issues/spec-review/freeze.
- **Scope of THIS repo (`agentd`)**: only the local runtime. **Specify-web is out of scope here** — it is its own repository/project. This doc records the seam so agentd builds the right hooks and does not over-reach.

P264 refines this boundary for enterprise execution. Where this document uses
the broad term "agentd" without distinguishing durable execution control from
worker-local process state, the enterprise ownership amendment is
authoritative. It also replaces the standalone-mode convenience wording below
with explicit `ProjectAuthorityPort` selection: a configured Specify authority
fails closed and never silently falls back to local authority.

---

## 0. Why this doc exists

The P0 design doc (`2026-05-29-agentd-design.md`) was written before the Robrix/Specify/agentd three-system split was nailed down. It put issues, spec review, freeze, and the canonical workflow state machine **inside** agentd. Under the confirmed Path B those belong to **Specify**. This doc is the reconciliation: what stays in agentd, what moves to Specify, and the protocol between them. Where this doc and the P0 design doc conflict, **this doc wins** and the P0 design/plans get patched (see §6 Delta).

---

## 1. Three-system architecture

```
┌────────────┐   Matrix    ┌─────────────────────────────┐   HTTP/SSE+ws   ┌──────────────────────────┐
│  Robrix    │ ◄─────────► │  Specify  (web, separate)   │ ◄─────────────► │  agentd  (local runtime) │
│ (chat UI)  │   protocol  │  = Matrix Application Svc   │   + webhook     │  = orchestration cli     │
└────────────┘             │  project context SoT:       │                 │  DOT engine, tmux, mempal│
   人发命令                │  issues/todo/bug/需求文档    │                 │  adversarial review      │
   /start /execute         │  spec versions + review     │                 │  reports semantic events │
   看通知                  │  + freeze + state machine   │                 └───────────┬──────────────┘
                           │  open API/SDK · CI · hosting│                             │ MCP / tmux
                           └─────────────────────────────┘                             ▼
                                       │ GitHub API                          ┌──────────────────────┐
                                       ▼ (Specify owns)                       │ Local Agents         │
                              ┌──────────────────┐                            │ claude-code / codex  │
                              │ GitHub (issues,  │                            └──────────────────────┘
                              │ PRs, repos)      │
                              └──────────────────┘
```

- **Robrix** — Matrix client. Humans issue `/start`, `/execute`, `/status`; receive notifications. No business logic.
- **Specify** — the Matrix Application Service. Owns the project context (issues / todo / bug / 需求文档), spec version management, the human review queue, the **freeze** operation, the canonical workflow state machine (`pending_spec_draft → … → done`), the open API/SDK, CI hooks, GitHub integration, and hosting/billing. Self-hostable, open source. **Separate repo.**
- **agentd** — the local runtime. Connects to the Matrix server via a bridge (to receive dispatch + post execution notifications) and to Specify via HTTP/SSE/ws (pull issue context, push spec drafts, pull frozen specs, report semantic events). Runs the DOT workflow engine, spawns local agents (tmux/MCP), runs adversarial review, opens PRs at execution time. **This repo.**

GitHub is owned by **Specify** (issues, repo metadata, PR status checks). agentd may still open a PR at execution time via a token Specify provisions, but issues come to agentd **through Specify**, not directly from GitHub.

---

## 2. The workflow splits in two (the crux)

The single `issue-to-pr.dot` from the P0 design splits at the human-review/freeze boundary, which now lives in **Specify**:

### Workflow 1 — DRAFT (agentd; triggered by Specify on `/start <issue>`)

```dot
start
  → fetch_issue_context   (tool: pull ACME-742 context from Specify API)
  → propose_spec          (codergen, role=spec-writer, local LLM)
  → lint_spec             (tool: agent-spec lint --min-score 0.7)
  → push_draft_to_specify (tool: POST /api/v1/specs/{id}/drafts on Specify)
  → done                  (agentd's part ends; run reports "draft pushed")
```

agentd does NOT review or freeze. It drafts, lints, pushes, exits the draft run.

### [ Specify-web owns the gap ]

```
pending_human_review  → team reviews/edits/approves in Specify web (HUMAN CAP #1)
                      → Specify FREEZE → immutable ACME-742-spec-v1.0
                      → ready_for_execution
```

### Workflow 2 — EXECUTE (agentd; triggered by Specify on `/execute <frozen-spec-id>`)

```dot
start
  → pull_frozen_spec      (tool: GET frozen spec + acceptance criteria from Specify)
  → draft_plan            (tool: agent-spec plan)
  → implement             (codergen, role=implementer, backend=tmux, worktree=auto)
  → verify_lifecycle      (tool: agent-spec lifecycle, goal_gate=true)
  → review                (parallel.fan_out, adversarial reviewers, blind, frozen bundle)
  → aggregate             (parallel.fan_in, majority_pass, goal_gate=true)
  → open_pr               (tool: gh pr create, via Specify-provisioned token)
  → report_acceptance     (tool: PUSH acceptance result + PR link to Specify)
  → done
```

Throughout Workflow 2, agentd streams semantic events to Specify (`workflow.started`, `task.claimed`, `criterion.passed`, `agent.blocked`, …). Specify drives `executing → pending_acceptance_review → done` (HUMAN CAP #2 in Specify/GitHub).

**Both workflows still use the full agentd engine** — DOT parse, handlers, park/resume, fan_out/fan_in, goal_gate, checkpoint. The adversarial review (Workflow 2) is exactly the engine we hardened. Nothing in agentd-core/tmux/store/mempal is wasted.

---

## 3. What `wait.human` means now

- **Spec approval** (CAP #1) — REMOVED from agentd. It happens in Specify web. agentd's old `propose_spec → lint → wait.human(/spec-approve) → finalize → git commit` sub-graph is replaced by `propose_spec → lint → push_draft_to_specify` (Workflow 1).
- **Execution-time human decision** (`agent.blocked` → `request_human_decision`) — KEPT in agentd. This is the rarer in-loop case where a running agent needs a human call mid-execution. It can surface via Specify (which relays to Robrix) or directly via Matrix. The `wait.human` handler + park/resume machinery we built stays for this.

So `wait.human` is not deleted — its dominant use (spec approval) moves out, its minor use (mid-execution decision) stays.

---

## 4. Protocol seam: agentd ↔ Specify

agentd is a **client** of Specify for project context, and a **reporter** to Specify for execution state. Specify is a **client** of agentd's surface only optionally (e.g. to trigger a draft). Concretely:

### 4.1 agentd → Specify (outbound, agentd initiates)

| Purpose | Call | When |
| --- | --- | --- |
| Pull issue context | `GET {specify}/api/v1/issues/{id}` | Workflow 1 `fetch_issue_context` |
| Push spec draft | `POST {specify}/api/v1/specs/{id}/drafts` | Workflow 1 `push_draft_to_specify` |
| Pull frozen spec | `GET {specify}/api/v1/specs/{id}/versions/{ver}` | Workflow 2 `pull_frozen_spec` |
| Report semantic event | `ws {specify}/…/events` or `POST …/workflows/{id}/events` | throughout Workflow 2 |
| Report acceptance | `POST {specify}/…/workflows/{id}/acceptance` | Workflow 2 `report_acceptance` |

agentd holds a **Specify workspace token** (scoped: `read:issues`, `write:drafts`, `read:specs`, `write:events`). This is the only new credential agentd needs.

### 4.2 Specify → agentd (inbound dispatch)

Dispatch is **pull-initiated from agentd's side** to honor the "Specify never pushes to the client machine" boundary, BUT in practice agentd is triggered via the Matrix channel (matrix bridge) the same way the P0 design already does:

- Specify (as MAS) posts a work-token / dispatch message to the Matrix channel on `/start` or `/execute`.
- agentd's matrix bridge listens, picks up the dispatch, and runs the matching workflow.
- agentd reports back via §4.1 (ws/HTTP to Specify), and Specify relays human-facing notifications to Robrix.

So agentd's existing `agentd-matrix` adapter stays, but its **role narrows**: it is a dispatch listener + notifier, NOT the command authority. `/start` `/execute` `/spec-approve` are parsed by **Specify** (the MAS), not agentd.

### 4.3 The seam is mostly agentd-surface (already planned)

agentd's planned `agentd-surface` crate (HTTP+SSE + MCP server, P0.7) already exposes events. The new pieces are small:
- an **outbound Specify client seam** (`agentd-specify` — pull issue, push draft,
  pull frozen spec, report events, report acceptance). As built through P145,
  this is an object-safe `SpecifyClient` trait with `OfflineSpecify`, protocol
  value types, test recording support, semantic event mapping, and runtime
  reporting through `ProductionRunHost`. Real HTTP/WS transport remains future
  work gated on a concrete Specify API contract.
- the semantic event mapping currently serializes durable agentd run events to
  Specify's first stable vocabulary (`agent.blocked`, `workflow.finished`,
  `workflow.failed`). Richer semantic events such as `task.claimed` or
  `criterion.passed` wait until the runtime emits those facts explicitly.

---

## 5. Ownership table

| Concern | Owner | agentd's relationship |
| --- | --- | --- |
| Issues / todo / bug / 需求文档 | **Specify** | pulls context via API; does not store as SoT (may cache per-run) |
| Spec draft authoring | **agentd** (also other entry points) | codergen produces draft, POSTs to Specify |
| Spec human review | **Specify** (web, CAP #1) | not involved |
| Spec freeze + immutable version | **Specify** | pulls the frozen version to execute |
| Canonical workflow state machine (`pending_spec_draft…done`) | **Specify** | reports semantic events that drive it; keeps its own *local execution* run state for the engine/checkpoint |
| Local execution run state + checkpoint/resume | **agentd** | owns `~/.agentd/agentd.db` for engine state only |
| DOT workflow engine, handlers, park/resume | **agentd** | core, unchanged |
| Adversarial review (fan_out/fan_in, bundle, stance packs) | **agentd** | core, unchanged |
| AgentBackend / tmux / local agent orchestration | **agentd** | core, unchanged |
| mempal (memory + cowork-bus) | **mempal** (dep) | agentd is MCP client, unchanged |
| Matrix Application Service (command authority) | **Specify** | agentd is a dispatch listener + notifier only |
| GitHub (issues, repos, PR status) | **Specify** | agentd may open a PR at execution time via Specify-provisioned token |
| Web collaboration UI / multi-project dashboard / API platform / hosting / billing | **Specify** | none |

---

## 6. Delta to the existing agentd P0 design/plans

This is what changes in THIS repo once Path B is adopted. **Nothing here is destructive to agentd-core/tmux/store/mempal** — the engine survives intact.

| # | Area | Current (P0 docs) | Change under Path B |
| --- | --- | --- | --- |
| Δ1 | Canonical workflow | one `issue-to-pr.dot` incl. spec review + freeze + git commit | split into `draft.dot` (Workflow 1) + `execute.dot` (Workflow 2); review+freeze removed (→ Specify) |
| Δ2 | `wait.human` for spec approval | in agentd via Matrix `/spec-approve` | removed; `wait.human` retained only for mid-execution `request_human_decision` |
| Δ3 | Issues | `issues` table = mirror of GitHub, agentd pulls from GitHub | issues pulled from **Specify** API; `issues` table becomes an optional per-run cache, not a GitHub mirror |
| Δ4 | Spec finalized location | agentd commits `specs/issue-N.spec.md` to git after `wait.human` approve | Specify owns frozen spec; agentd pulls it. (git copy optional, Specify-driven) |
| Δ5 | Matrix adapter (P0.6) role | MAS + slash router (`/start`,`/execute`,`/spec-approve`) + wait.human delivery | narrowed to dispatch listener + execution notifier; slash authority → Specify |
| Δ6 | GitHub adapter (P0.5) | agentd owns GitHub read + webhook + status push | GitHub owned by Specify; agentd keeps only execution-time PR open via provisioned token. P0.5 may shrink or move |
| Δ7 | New: Specify client | — | `agentd-specify` exists as an optional no-network seam: `SpecifyClient`, `OfflineSpecify`, protocol value types, event reporting helper, and runtime reporting hook are in place; real HTTP/WS transport/auth/config wait on the external Specify API contract. |
| Δ8 | Semantic events | EventBus `EventKind` internal | first semantic event mapping is in place for durable run events (`agent.blocked`, `workflow.finished`, `workflow.failed`); richer event facts are deferred until runtime emits them explicitly. |
| Δ9 | Design doc framing | "agentd is the daemon that owns issues/spec/review" | reframe §0/§1/§5/§7 to "agentd is the local runtime; Specify is the web layer"; this boundary doc is authoritative |

**Phasing note**: Δ1–Δ5 touch P0 (workflow + Matrix + storage). Δ6–Δ8 are P1 (the Specify seam can be stubbed in P0 — agentd can run standalone with a local frozen spec for the MVP demo, and the Specify client lands in P1). Δ9 is a doc pass.

---

## 7. agentd can still run standalone (important)

Because Specify is a *layer on top*, agentd must remain runnable WITHOUT Specify (the Solo/self-host case, and all of P0 development). In standalone mode:
- issue context comes from a local file or GitHub directly (dev convenience),
- spec draft is written to a local path (no Specify push),
- "freeze" is a local git tag / file,
- no semantic events are shipped (or shipped to a local sink).

The Specify client (`agentd-specify`, P1) is an **optional adapter**. Today it
defaults to `OfflineSpecify` and runtime reporting is best-effort, so standalone
agentd keeps running without Specify. Future online mode should be gated behind
explicit config once the real Specify API contract exists. This preserves the P0
plan's ability to build + demo agentd on its own, and matches Specify's own
"Solo $0 self-deploy" tier.

---

## 8. Out of scope for this repo

Specify-web itself — its web UI, issue center, review queue, freeze state machine, open API server, billing, hosting — is a **separate project/repository**. This repo (`agentd`) only builds: the local runtime + the thin outbound Specify client (`agentd-specify`, P1). Do not implement Specify-web here.

---

## 9. Open questions (carry into the design-doc patch)

- **Q1** Does agentd expose MCP `post_action` tokens like `specify.publish_draft` / `specify.mark_executed`, or is the Specify client purely a set of `tool` nodes in the DOT? (Lean: `tool` nodes — keeps the seam in the workflow, not hard-coded.)
- **Q2** Where does the user trigger spec drafting — `/start` in Robrix → Specify → dispatch to agentd, or a "Generate draft" button in Specify web → Specify → dispatch to agentd? (Both end at the same agentd Workflow 1; UX differs, agentd side identical.)
- **Q3** Does Specify store full spec content or only metadata + a pointer to agentd/git? (Lean: Specify stores the canonical frozen spec content — it is the SoT for specs per the product doc; agentd holds the working draft + pulls the frozen copy.)
- **Q4** Standalone-mode freeze semantics (git tag vs local file) — define when implementing Δ4.
