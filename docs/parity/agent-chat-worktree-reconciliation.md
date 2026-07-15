# Agent-Chat Worktree Reconciliation

Status: decided by P263 on 2026-07-10.

## Integration Authority

The current `/Users/zhangalex/Work/Projects/AI/agentd` worktree is the
integration authority. Its P200-P262 specs are the authoritative implementation range.
The sibling `agentd-agent-chat-replacement` worktree shares the same Git
HEAD but contains a conflicting uncommitted P202-P228 sequence; it is evidence
for capability porting, not a branch or migration stream that can be merged
blindly.

P200 and P201 are common history by artifact name. Every sibling P202-P228 item
has exactly one disposition below. `covered_by_base` requires concrete base
spec evidence, `port_required` remains blocking work, `integrated_as_p263`
means its parity-freeze intent is incorporated here, and `renumbered` reserves
a new non-conflicting base id.

## Spec Mapping

| Source spec | Source capability | Base evidence or gap | Disposition | Base destination |
| --- | --- | --- | --- | --- |
| p202 | registry HTTP lifecycle | p213 registry lifecycle | covered_by_base | p213 |
| p203 | agentctl registry lifecycle | p213 agentctl agent commands | covered_by_base | p213 |
| p204 | runtime spawn HTTP and CLI | p214 start plus p234 lifecycle | covered_by_base | p214+p234 |
| p205 | runtime status, capture, shutdown, rebind | p234 covers down/rebind but not status/capture parity | port_required | p272 |
| p206 | registry-backed task assignment | p229 task-graph and p230 workflow scheduler allocation | covered_by_base | p229+p230 |
| p207 | pool view | p228 durable pool scheduler | covered_by_base | p228 |
| p208 | reservation release | p228 release and queue drain | covered_by_base | p228 |
| p209 | provision plan | p228 structured provision result | covered_by_base | p228 |
| p210 | provision-registration reconciliation | no base registration reconciliation for provision reservations | port_required | p273 |
| p211 | Codex auto-spawn after provision | no automatic scheduler-to-runtime spawn path | port_required | p273 |
| p212 | durable dispatch queue | p228 queue plus p232/p233 wakeup | covered_by_base | p228+p232+p233 |
| p213 | durable direct inbox | p217 direct inbox | covered_by_base | p217 |
| p214 | live task CRUD | p226 live task CRUD | covered_by_base | p226 |
| p215 | live task graph | p227 live task graph | covered_by_base | p227 |
| p216 | JSON import and shadow | p216 agent plus p224 message plus p225 task import | covered_by_base | p216+p224+p225 |
| p217 | remote server heartbeat | p235 relay server heartbeat | covered_by_base | p235 |
| p218 | relay stream and delivery events | p235 stream and delivery-event audit | covered_by_base | p235 |
| p219 | unread backfill and push-delivered acknowledgement | p235 lacks unread-list, push ack, and message delivery lookup | port_required | p274 |
| p220 | direct message suppression | base direct inbox has no durable suppression behavior | port_required | p275 |
| p221 | group inbox | p220 durable groups and group history | covered_by_base | p220 |
| p222 | MCP group tools | p220 post, check_group, and identity binding | covered_by_base | p220 |
| p223 | parity freeze and enterprise gap audit | reconciled against the P200-P262 base | integrated_as_p263 | p263 |
| p224 | Specify, agentd, and OpenFab ownership | not implemented in base | renumbered | p264 |
| p225 | enterprise runtime and worker identity | not implemented in base | renumbered | p265 |
| p226 | project-room-repository authority references | not implemented in base | renumbered | p266 |
| p227 | enterprise agent, worker, and runtime store | adapt to base schema after migration 0012 | renumbered | p267 |
| p228 | enterprise artifact and audit store | adapt after P267 without copying source migration numbers | renumbered | p268 |

The exact port-required source set is P205, P210, P211, P219, and P220. No
enterprise contract may be used to imply those compatibility behaviors are
covered.

## Migration Authority

Base migrations `0001-0012` are authoritative. Sibling migrations are never copied by version,
even when a filename differs, because SQLx migration identity
is the numeric prefix and deployed base databases already own that history.

- P267 adapts the enterprise agent/worker/runtime model as base migration
  `0013`.
- P268 adapts the enterprise artifact/audit model as base migration `0014`.
- Compatibility ports use later additive base migrations only when their own
  specs prove a schema change is necessary.

Migration SQL must be rewritten against the base table/column contracts and
verified both on a fresh database and by applying the new migration after the
real base `0012_matrix_bridge_contract.sql`. Source SQL is evidence, not an
applicable migration chain.

## Consequence

Do not merge, rebase, delete, or clean either dirty worktree as part of P263.
Port one capability/spec at a time into the base namespace and run its full
agent-spec lifecycle. P263 changes no production behavior and does not make
agentd a complete agent-chat replacement.
