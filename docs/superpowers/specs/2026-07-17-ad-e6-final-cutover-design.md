# AD-E6 Final Cutover Design

Status: approved by the canonical AD-E roadmap and the user's instruction to
continue implementation without intermediate acceptance gates.

## Decision

Agentd will use a durable, restartable cutover run rather than a sequence of
unrecorded shell commands. Agent-chat is accepted only as an offline import
source. Native agentd services own command ingress, cursor authority, task
execution, process sessions, and operator recovery after activation.

Two alternatives were rejected:

1. A big-bang copy followed by immediate deletion cannot explain partial
   failure or perform deterministic rollback.
2. A long-lived production feature flag for agent-chat/tmux preserves the dual
   authority AD-E6 is intended to remove.

## Cutover State

Migration `0022` adds immutable source snapshots, stable legacy-to-native ID
mappings, shadow decision observations, cutover runs, ordered step receipts,
cursor handoffs, backup manifests, service installations, and rollback records.
Every mutating operation requires a cutover id and idempotency key. Repeating
the same key with different content is a conflict.

The state machine is:

`planned -> importing -> shadowing -> draining -> handoff_ready -> active -> retired`

Rollback is legal from `importing` through `active` and records a terminal
`rolled_back` state. Activation requires an exact source digest, a drift-free
shadow report, no nonterminal legacy work, and acknowledged Matrix cursor
handoffs.

## Import And Shadow

One import command snapshots all supported agent-chat JSON files, calculates a
canonical digest, imports agents/messages/groups/cursors/tasks/task graphs in a
single cutover transaction boundary, and writes stable mapping rows. Original
legacy ids remain compatibility aliases; native ids and digests are immutable.

Shadow comparison evaluates normalized decisions, not only row existence. It
compares agent routing, message audience, task status/assignee, graph runnable
nodes, and cursor positions. Raw prompts, message bodies, tokens, and
transcripts are not copied into comparison evidence.

## Operations

`agentctl cutover` owns plan, import, shadow, drain, handoff, activate, inspect,
rollback, doctor, backup, restore, and service rendering. Doctor emits bounded
structured checks for database/schema, project authority, workers, leases,
queue, runtime, Matrix, OpenFab, artifacts, backup freshness, and cutover
authority. Backup uses SQLite's online backup operation plus a SHA-256 manifest;
restore refuses a running service and verifies the manifest before atomic
replacement.

Service rendering produces launchd for local use, systemd plus Compose for a
team server, and a fleet environment contract consumed by AD-E7. Generated
services start agentd/Matrix/OpenFab-native paths only.

## Runtime Removal

The git worktree allocator moves to `agentd-worktree`. Production daemon
composition uses a native PTY agent backend and `NativeRuntimeService` recovery;
`agentd-bin` no longer depends on `agentd-tmux`. The tmux crate, tmux smoke
scripts, production flags, and operator procedures are removed. Legacy tmux
fields may be read only by the offline importer and translated into historical
mapping evidence; they cannot select a runtime.

## Failure Rules

- Import and handoff fail closed on source digest drift.
- Activation is transactional and cannot coexist with legacy write authority.
- Backup/restore never overwrite an unverified database.
- Doctor reports unavailable dependencies without leaking credentials or raw
  execution content.
- Rollback restores authority/cursors from immutable receipts; it does not
  resurrect a tmux process.

## Verification Deferral

Tests and operator drills are authored with the implementation but are not run
until AD-E1 through AD-E7 candidate code is present. The single final checklist
contains import replay, decision drift, in-flight drain, cursor handoff,
service installation, backup/restore, rollback, no-tmux dependency, and
Codex-only real execution evidence.
