# AD-E0 Main Reconciliation Rehearsal

- Rehearsal date: 2026-07-13
- Candidate input: `12e1b66` on `agentd/tr_01KWWTVEK1AC6C836SXSP7Y3Q3`
- Main conflict-rehearsal input: `096e37c` on local `main`
- Latest remote main inspected: `ce7f18f` on `origin/main`
- Merge base: `d2b9fff081a73871615264760d48acb71e82e4f7`
- Isolated worktree: `/Users/zhangalex/Work/Projects/AI/agentd-ad-e0-reconcile`
- Rehearsal branch: `agentd/ad-e0-reconcile-rehearsal`
- State: no-commit merge rehearsal; no push or pull request
- Verdict: **RECONCILIATION PASS; MAIN INTEGRATION REMAINS HOLD**

## 1. Purpose

This report supersedes the current-state conclusions in the 2026-07-12 HOLD
review. It records the candidate-versus-main merge rehearsal, the authority
composition decision, repository verification, and the remaining external
gates. It does not convert candidate code into `main`, sign FSF-0, or approve a
release.

## 2. Canonical Lineage

P263 remains the canonical mapping record. The former sibling-worktree P224-P228
enterprise sequence is retired and renumbered to P264-P268; it is not an
independent executable lineage. P269-P271 extend that same canonical lineage.
The existing base P224-P228 compatibility specs keep their unrelated message,
task, graph, and scheduler meanings.

The candidate commit chain is:

| Commit | Canonical scope |
| --- | --- |
| `85518ad` | P263 worktree reconciliation |
| `c22b402` | P264 ownership boundary |
| `0ea1472` | P265 identity contract |
| `76ae37d` | P266 project authority references |
| `9634104` | P267 runtime/worker store |
| `cbd0993` | P268 artifact/audit store |
| `670ad0e` | P269 project authority API |
| `d9d05dd` | P270 durable lease and fencing API |
| `9ec92bb` | P271 execution evidence APIs |

## 3. Reconciliation Result

Before the rehearsal, local `main...candidate` contained 35 main-only commits and 13
candidate-only commits. `git merge --no-commit --no-ff main` produced 14
conflicted files across workspace manifests, daemon composition, host/MCP
surfaces, and their tests. All conflicts were resolved in the isolated worktree;
`git diff --name-only --diff-filter=U` returns zero paths.

The Codex-only remote preflight then fetched `origin/main@ce7f18f`, which is two
commits ahead of local main and contains only two additive paths:
`docs/agentd-real-execute-smoke.md` and
`crates/agentd-bin/tests/real_execute_smoke_artifact.rs`. Their bytes were
incorporated exactly from `origin/main`; they do not overlap the 14 conflict
paths. The final integration must still merge from the then-current remote tip,
not rely on this no-commit rehearsal metadata.

The resolved candidate preserves both main-line behavior and the enterprise
lineage:

- `agentd-project-authority` owns the sole project-authority domain contract.
- Its Local and Specify authority adapters preserve explicit composition,
  validation, snapshot pinning, bounded recovery, and fail-closed configured
  Specify behavior.
- `agentd-specify` remains an outbound protocol seam for issue/spec/event and
  acceptance operations. It does not define a second authority abstraction.
- The workspace keeps both crates, but the daemon currently wires only the
  optional `agentd-specify` semantic-event seam. The
  `agentd-project-authority` adapters preserve fail-closed authority behavior,
  but production daemon composition of `ProjectAuthorityPort` remains future
  work. Optional semantic event reporting is best-effort after durable agentd
  event persistence.
- No production HTTP/WS Specify adapter is claimed because no ratified wire
  contract exists in this repository.
- Main-line initial context, checkpoint metadata, atomic outcome/checkpoint,
  SSE sanitization, workflow-change approval, and human-answer MCP behavior are
  preserved alongside candidate auth, scheduler, lifecycle, Matrix, and
  P264-P271 behavior.
- Candidate P205 intentionally supersedes the older start-run error propagation:
  launch failures are persisted and emitted as structured `RunProgress::Failed`
  instead of being returned only as an error.
- The aggregate real-environment helper uses an explicit four-role Codex runtime
  matrix. It does not invoke the standalone Claude smoke helper.

The code/spec/script diff from candidate `HEAD`, excluding governance reports,
has SHA-256
`05a5f8b08dbec8a0a5723090976f4e8384c835e1a9aa4f6ebf822b1fab172ec1`.
This digest identifies this uncommitted rehearsal only; it is not a Git commit.

## 4. Verification

The reconciled worktree passed:

- `cargo test --workspace`;
- `cargo clippy --workspace --all-targets -- -D warnings`;
- `cargo fmt --all --check`;
- `git diff --check`;
- focused production-host tests for initial context and assignment metadata;
- focused MCP tests for `submit_human_answer` delivery and stale-wait handling;
- focused runtime tests for durable-before-Specify reporting, event dedup, and
  best-effort Specify failure handling.
- exact selector restoration for P139, P141, P145, and P150, plus P70/P141
  nine-tool and durable-inbox contract alignment; each affected lifecycle
  passes at quality 1.0 without relying on a zero-test selector;
- exact-byte comparison and both tests for the `origin/main@ce7f18f` real-execute
  smoke artifact;
- Codex-only real-execute preflight with
  `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex`.

The real execute preflight passed against `origin/main@ce7f18f`. The full
`--execute` path was not run from the no-commit rehearsal because that harness
publishes an implementation branch and opens a real PR from Git `HEAD`; doing so
before an exact reconciliation commit would test and publish the wrong bytes.

P264-P271 each parsed, linted at quality 1.0, and passed full lifecycle against
the reconciled worktree:

| Spec | Scenarios | Lifecycle |
| --- | ---: | --- |
| P264 | 8 | PASS |
| P265 | 8 | PASS |
| P266 | 8 | PASS |
| P267 | 10 | PASS |
| P268 | 9 | PASS |
| P269 | 8 | PASS |
| P270 | 10 | PASS |
| P271 | 8 | PASS |

Structured lifecycle logs are at
`/tmp/agentd-ad-e0-lifecycle-20260713` for this local rehearsal. They are
ephemeral supporting evidence, not committed acceptance records.

Independent Codex review `019f5858-8f50-7973-ada0-79dee9653680` first found
three Important and two Minor reconciliation issues. After correcting the
daemon-composition claim, restoring six exact selectors, aligning P70/P141 to
the nine-tool registry and durable inbox, updating the P205 supersession note,
and rerunning verification, the final exact-byte verdict was PASS with no
remaining or new Critical/Important findings.

## 5. External Gate Changes

The OpenFab PRD amendment and ADR gate is now satisfied in the inspected OpenFab
worktree:

- `docs/OpenFab_MVP_Design_and_PRD.md` records the enterprise agentd execution
  profile as design-approved; SHA-256
  `683e8b37ecc9d6f29804aacff648e64a2897364c275f0f7d8e156f8d94f41892`.
- `docs/adr/0003-agentd-execution-and-openfab-certification-boundary.md`
  records both decision-owner approvals; SHA-256
  `366949f5efcf537cda6dd13112bb0ace2bf6ba908594ac7c566e402170e32ebc`.

FSF-0E machine evidence is complete at OpenFab `1ae4394`, but the evidence file
explicitly retains candidate status and says the FSF-0 record remains
`NOT ACCEPTED`. Its SHA-256 is
`de033bf6dd1664de45108bb9d73c2c5fa50a959ba1bcfe3a9dedc63d92f692f3`.
The Robrix walkthrough and accountable human signature are still required.

## 6. Gate Matrix

| Gate | State | Evidence or remaining action |
| --- | --- | --- |
| Canonical P263-P271 lineage | PASS | committed mapping and candidate chain |
| No duplicate authority | PASS in rehearsal | one `ProjectAuthorityPort`; Specify protocol seam remains outbound |
| Main reconciliation | PASS in rehearsal | 14 conflicts resolved; full mechanical verification passed |
| P264-P271 lifecycle | PASS | 69 scenarios total, quality 1.0 |
| OpenFab PRD/ADR decomposition | PASS in inspected worktree | approved PRD amendment and ADR 0003 |
| FSF-0 machine evidence | PASS candidate | FSF-0E real E2E evidence complete |
| FSF-0 accountable acceptance | HOLD | Robrix walkthrough and human signature pending |
| Independent exact-byte review | PASS for rehearsal bytes | Codex review `019f5858-8f50-7973-ada0-79dee9653680`; digest `05a5f8b0...172ec1` |
| Remote protected checks | PENDING | no review branch or PR has been published |
| Human integration approval | PENDING | required after all preceding evidence is bound to an exact commit |

## 7. Decision

AD-E0's local engineering reconciliation work is complete enough for independent
review and an exact-commit candidate to be prepared. It is not authorized for
main integration while the versioned FSF-0 acceptance, exact-byte review,
protected checks, and final human integration approval are absent. Missing
external evidence remains visible as HOLD and must not be converted into an
implicit pass.
