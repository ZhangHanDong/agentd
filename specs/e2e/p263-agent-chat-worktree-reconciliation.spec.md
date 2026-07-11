spec: task
name: "agent-chat worktree namespace and capability reconciliation"
tags: [e2e, parity, agent-chat, reconciliation, enterprise, roadmap, p263]
---

## Intent

Establish the current `/Users/zhangalex/Work/Projects/AI/agentd` worktree as the
single integration baseline before enterprise replacement work continues. The
slice must reconcile every conflicting P202-P228 source spec and migration from
the sibling `agentd-agent-chat-replacement` worktree without blindly merging,
dropping verified base behavior, reusing occupied ids, or claiming replacement
completion.

## Decisions

- The base worktree's verified P200-P262 sequence is the authoritative
  implementation range. The sibling replacement worktree is read-only source
  evidence until a capability is mapped, ported under a new id when needed, and
  reverified here.
- Add `docs/parity/agent-chat-worktree-reconciliation.md` with exactly one row
  for each conflicting sibling source spec P202-P228. Rows use
  `covered_by_base`, `port_required`, `integrated_as_p263`, or `renumbered`.
- The uncovered sibling behaviors are exactly P205 runtime status/capture,
  P210 provision-registration reconciliation, P211 Codex auto-spawn, P219
  unread/push-delivered acknowledgement, and P220 message suppression. They
  remain explicit future work rather than being inferred covered.
- Sibling P223 becomes this P263 reconciliation/freeze. Sibling P224-P228 are
  renumbered to P264-P268 for ownership, identity, project-authority references,
  enterprise agent/worker/runtime storage, and artifact/audit storage.
- Base migrations `0001` through `0012` are authoritative. Sibling migration
  files are never copied by version. P267 adapts enterprise runtime storage as
  migration `0013`; P268 adapts artifact/audit storage as migration `0014`.
- Amend the current parity map with enterprise/native replacement requirements
  while preserving all P200-P262 evidence and statuses. Required incomplete
  rows remain blocking under the existing `agentctl parity audit` exit-1
  behavior; P263 adds no second gate command.
- Amend the current replacement roadmap with non-conflicting P263-P279 planned
  ids. P264 resolves Specify/agentd/OpenFab ownership before P267/P268 schema
  work, and the immediate next step is P264.
- P263 changes documentation and artifact tests only. It does not port sibling
  production code, add migrations, alter APIs, or mutate either worktree's
  existing spec/run evidence.

## Boundaries

### Allowed Changes

- specs/e2e/p263-agent-chat-worktree-reconciliation.spec.md
- docs/parity/agent-chat-worktree-reconciliation.md
- docs/parity/agent-chat-capability-map.md
- docs/plans/2026-07-08-agent-chat-replacement-roadmap.md
- docs/superpowers/plans/2026-07-10-p263-worktree-reconciliation.md
- crates/agentctl/tests/worktree_reconciliation_contract.rs

### Forbidden

- Do not modify sibling worktree files or copy sibling migration SQL/code into
  the base worktree in P263.
- Do not rename or rewrite P200-P262 specs, migrations, run logs, APIs, schema,
  runtime behavior, Matrix behavior, or parity evidence.
- Do not classify a source capability as covered without a concrete base spec
  reference, and do not hide a port-required behavior under an enterprise item.
- Do not add dependencies, CLI commands, HTTP/MCP routes, store modules, or
  database migrations.
- Do not start Claude, Codex, tmux, Matrix, systemd, launchd, remote relay, or
  other external processes in tests.
- Do not claim full agent-chat replacement while required parity rows remain
  partial or missing.

## Out of Scope

- Porting P205, P210, P211, P219, or P220 behavior.
- Implementing P264-P279 ownership, schemas, APIs, native runtime, worker
  protocol, leases, policy/quota, artifact upload, or doctor behavior.
- Committing, merging, rebasing, deleting, or cleaning either dirty worktree.
- Changing the real execute or Matrix smoke opt-in state.

## Completion Criteria

<!-- lint-ack: decision-coverage — five scenarios bind the complete source mapping, namespace/migration authority, enterprise parity rows, roadmap ordering, and current cutover gate behavior. -->
<!-- lint-ack: observable-decision-coverage — P263 outputs are repository Markdown/spec artifacts and the existing parity CLI result, all bound to tests. -->
<!-- lint-ack: output-mode-coverage — P263 adds no output mode; the existing parity audit output remains covered by its established CLI test. -->
<!-- lint-ack: boundary-entry-point — P263 has no production entry point; artifact tests inspect every changed contract and the existing CLI test binds parity behavior. -->
<!-- lint-ack: bdd-rule-grouping — all scenarios prove the single cross-worktree reconciliation rule. -->

Scenario: reconciliation rejects duplicate or missing sibling source mappings
  Test:
    Package: agentctl
    Filter: p263_reconciliation_maps_every_conflicting_source_spec
  Level: artifact inspection
  Test Double: repository Markdown file
  Given sibling source specs P202 through P228 conflict with the base namespace
  When the reconciliation table is parsed
  Then every source id must appear exactly once and duplicates or omissions fail
  And the port-required ids are exactly P205, P210, P211, P219, and P220
  And sibling P223-P228 map to base P263-P268 in order

Scenario: reconciliation reserves non-conflicting spec and migration sequences
  Test:
    Package: agentctl
    Filter: p263_reconciliation_reserves_non_conflicting_sequences
  Level: artifact inspection
  Test Double: repository files
  Given base specs through P262 and migrations through 0012
  When the reconciliation and current e2e spec namespace are inspected
  Then base P200-P262 remain authoritative and numerically unique
  And P263 is the only new implemented id
  And P267/P268 reserve migrations 0013/0014 without copying sibling versions

Scenario: parity map adds enterprise requirements without claiming completion
  Test:
    Package: agentctl
    Filter: p263_parity_map_adds_enterprise_requirements_without_completion_claim
  Level: artifact inspection
  Test Double: repository Markdown file
  Given the P200-P262 capability evidence remains authoritative
  When the P263 parity map is parsed
  Then all nine enterprise/native requirement ids exist
  And each new row is partial or missing
  And real Codex execution remains covered while Matrix remains partial

Scenario: roadmap orders ownership before enterprise schemas and advances to P264
  Test:
    Package: agentctl
    Filter: p263_roadmap_orders_reconciled_enterprise_work
  Level: artifact inspection
  Test Double: repository Markdown file
  Given P263 reconciles the parallel worktrees
  When the enterprise roadmap amendment is inspected
  Then planned ids P264-P279 are unique and named
  And ownership P264 precedes schema P267 and P268
  And the Immediate Next Step is P264 ownership

Scenario: existing parity audit remains the blocking cutover command
  Test:
    Package: agentctl
    Filter: parity_audit_reports_required_gaps_from_map
  Level: CLI integration
  Test Double: local agent-chat checkout path
  Given the reconciled parity map contains required partial and missing rows
  When `agentctl parity audit --agent-chat <path>` runs
  Then it exits `1`
  And it reports current required gaps without mutating agent-chat
