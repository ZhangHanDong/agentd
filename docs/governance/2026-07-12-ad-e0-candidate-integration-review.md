# AD-E0 Candidate Integration Review

> Superseded for current status by
> `docs/governance/2026-07-13-ad-e0-main-reconciliation-rehearsal.md`.
> This file remains the pre-reconciliation HOLD record.

- Review date: 2026-07-12
- Candidate branch: `agentd/tr_01KWWTVEK1AC6C836SXSP7Y3Q3`
- Candidate tip: `3c27424`
- Compared integration branch: `main` at `096e37c`
- Merge base: `d2b9fff`
- Verdict: **HOLD - NOT AUTHORIZED FOR MAIN INTEGRATION**

## 1. Scope

This review covers the P200-P271 compatibility and enterprise candidate
baseline, the canonical AD-E roadmap, and the AD-E1 minimum security design. It
does not certify FSF-0, approve the OpenFab PRD/ADR amendment, or substitute for
human code review.

## 2. Candidate Commit Manifest

| Commit | Scope | Candidate status |
| --- | --- | --- |
| `f0f3acc` | P200-P262 compatibility baseline | Reviewable feature-branch commit |
| `85518ad` | P263 worktree reconciliation | Reviewable feature-branch commit |
| `c22b402` | P264 ownership boundary | Reviewable feature-branch commit |
| `0ea1472` | P265 identity contract | Reviewable feature-branch commit |
| `76ae37d` | P266 authority references | Reviewable feature-branch commit |
| `9634104` | P267 runtime/worker store | Reviewable feature-branch commit |
| `cbd0993` | P268 artifact/audit store | Reviewable feature-branch commit |
| `670ad0e` | P269 project-authority API | Reviewable feature-branch commit |
| `d9d05dd` | P270 leases and fencing | Reviewable feature-branch commit |
| `9ec92bb` | P271 execution-evidence APIs | Reviewable feature-branch commit |
| `b6cfa03` | canonical AD-E/FSF roadmap | Reviewable feature-branch commit |
| `3c27424` | AD-E1 minimum security design/spec | Design-only feature-branch commit |

No entry in this table is a main-branch integration, release, FSF acceptance, or
enterprise readiness claim.

## 3. Verification Evidence

The candidate worktree passed the following before the roadmap/security design
commits were added:

- `cargo test --workspace`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all --check`;
- P271 agent-spec lifecycle and 8/8 scenario verification.

After the roadmap alignment commit:

- `cargo test -p agentctl --tests` passed, including 66 parity tests and the
  P263-P271 artifact contracts;
- P272 remained design-only and its agent-spec parsed/linted at 100%;
- the AD-E1 minimum security agent-spec parsed/linted at 100% with 13 scenarios.

These commands prove the candidate worktree state only. They do not prove that
the candidate compiles or behaves correctly after reconciliation with current
`main`.

## 4. Review Findings

### BLOCKER 1: FSF-0 acceptance record does not exist

The OpenFab factory roadmap requires a versioned FSF-0 acceptance record with
repository revisions, commands, results, artifact digests, exceptions,
accountable owner, and human sign-off. The inspected OpenFab worktree contains a
July 6 operational checklist with unchecked items, but no completed acceptance
record. Six FSF-0 task contracts parse/lint at 100%, but their critical E2E test
filters were not found in the inspected OpenFab, agent-chat, or Robrix2
implementations. Therefore FSF-1/AD-E0 cannot exit.

### BLOCKER 2: OpenFab PRD/ADR decomposition is not ratified

`docs/OpenFab_MVP_Design_and_PRD.md` still describes OpenFab dispatch through
`BasePort` without the ratified agentd decomposition. The enterprise factory
roadmap is a proposed, uncommitted document. A proposed ADR can record the
decision, but main integration remains blocked until the PRD and ADR are
approved by the accountable human owners.

### BLOCKER 3: candidate and main have diverged materially

`git rev-list --left-right --count main...3c27424` reports:

- 35 commits only on `main`;
- 12 commits only on the candidate branch.

The branches overlap in 21 modified paths, including `Cargo.toml`, agentd-bin
composition, surface HTTP/MCP code, core test support, and the Specify boundary
document. Current `main` contains `crates/agentd-specify`; the candidate branch
contains `crates/agentd-project-authority`. Their relationship has not been
reconciled. A direct merge or rebase without an authority-adapter decision can
silently discard newer Specify behavior or create two competing project
authority implementations.

### BLOCKER 4: no current remote review branch

The local branch reports its former upstream as gone. There is no current remote
candidate ref or pull request containing `3c27424`, so independent review and
protected-branch checks cannot run against these exact bytes.

### HIGH 1: lifecycle evidence is not yet curated

Agent-spec run JSON and checkpoint files exist as untracked/generated worktree
state. P271 has explicit passing lifecycle evidence, but P263-P270 evidence has
not yet been reduced to a committed manifest of spec digest, commands, result,
and artifact references. Task 4 must classify this state and retain only durable
evidence.

### HIGH 2: baseline review blast radius is large

The candidate differs from `main` by approximately 75,000 inserted lines across
256 files. `f0f3acc` intentionally consolidates P200-P262 into one baseline
commit. Integration review must therefore use capability/spec groupings and
path ownership, not a single undifferentiated PR approval.

## 5. Confirmed Invariants

- P224-P228 are retired as an executable lineage; P263-P271 are canonical
  candidates.
- Migrations `0013`, `0014`, and `0015` are additive after the base `0012`.
- Enterprise migrations add no agentd-owned organization, team, project,
  repository, room-binding, RBAC, or quota tables.
- P269 fails closed for configured Specify and selects local authority only by
  explicit composition.
- P270 uses durable lease ids and task-scoped fencing tokens rather than
  scheduler tickets.
- P271 validates P270 claims before worker evidence and records rejection audit.
- P272-P275 remain paused FSF-0 transitional candidates.
- AD-E1 implementation remains blocked until this AD-E0 gate is satisfied.

## 6. Required Integration Sequence

1. Produce and human-sign the FSF-0 acceptance record.
2. Ratify the OpenFab PRD amendment and ADR preserving `BasePort` while assigning
   durable execution to agentd.
3. Publish the exact candidate tip to a review branch.
4. Reconcile current `main` into a clean candidate worktree, explicitly deciding
   how `agentd-specify` implements or composes with `ProjectAuthorityPort`.
5. Resolve all 21 overlapping paths without dropping either main-line behavior
   or P200-P271 contract coverage.
6. Run full workspace tests, strict Clippy, formatting, migration fresh/backcompat
   tests, agent-spec lifecycle verification, and targeted real Codex smoke.
7. Record independent review verdicts and protected-branch results.
8. Merge only after all gate rows below are true.

## 7. Gate Matrix

| Gate | Required evidence | Current state |
| --- | --- | --- |
| Canonical lineage | P263 mapping plus commits | PASS |
| Candidate verification | tests, Clippy, fmt, lifecycle | PASS for candidate tip before main reconciliation |
| No duplicate authority | reconciled Specify/ProjectAuthority adapters | FAIL |
| FSF-0 acceptance | signed versioned acceptance record | MISSING |
| PRD/ADR | approved amendment and ADR | MISSING |
| Main reconciliation | clean merge/rebase rehearsal and full verification | MISSING |
| Independent review | exact-tip reviewer verdicts | MISSING |
| Remote protected checks | published branch/PR checks | MISSING |
| Human integration approval | accountable sign-off | MISSING |

The integration decision is fail-closed. Missing evidence is a failed gate, not
an implied approval.

## 8. Current Decision

Keep `3c27424` as a reviewable local candidate. Do not merge, rebase, force-push,
or delete either dirty worktree during this review. Main integration becomes an
allowed action only after the gate matrix is updated with real evidence and an
accountable human changes the verdict from `HOLD` to `APPROVED`.
