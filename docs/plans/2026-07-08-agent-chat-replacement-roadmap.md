# agentd — agent-chat replacement roadmap

> Status: planning. Goal: make agentd a practical replacement for
> `/Users/zhangalex/Work/Projects/consult/agent-chat`, without losing the parts
> agent-chat already solves in daily use.
>
> This is intentionally a replacement roadmap, not one monolithic implementation
> plan. Each phase below should become its own spec/plan before code starts.

## 0. Replacement target

agentd can replace agent-chat only when it covers both layers:

1. **Workflow execution runtime**: spec/issue -> plan -> implementation agents ->
   independent review -> aggregate -> publish/open PR, with restart-safe state.
2. **Multi-agent coordination product**: agent registry, start/offline/heartbeat,
   DM/group inbox, pool scheduling, dashboard/CLI operations, optional Matrix and
   remote relay, and migration from existing agent-chat state.

agentd already has a stronger foundation for the first layer. It does not yet
cover enough of the second layer to replace agent-chat.

Update 2026-07-09: G1/Phase B is now proven on this host by
`p204-codex-matrix-r9`: a Codex-only real execute run reached `finished` after
implement, lifecycle verification, three passing Codex reviewers,
publish/open-pr, and acceptance reporting. Replacement remains blocked on the
coordination product layers in Phases C-H.

## 1. Non-negotiable cutover gates

- **G1 real execution gate**: `scripts/agentd_real_execute_smoke.sh --execute`
  must pass with real agents, real reviewers, real verification, and either a
  real PR or a precise local preflight blocker. Codex must be a first-class
  supported runtime; Claude cannot be required for the smoke.
- **G2 compatibility gate**: every agent-chat capability marked "required" in
  the parity map has an agentd endpoint, CLI command, or explicit migration
  replacement.
- **G3 migration gate**: agent-chat state can be imported or shadowed without
  deleting the original JSON stores.
- **G4 operations gate**: agentd can run the local stack, recover after daemon
  restart, show live status, and explain blocked runs without reading raw logs.
- **G5 shadow gate**: at least one real project runs agent-chat and agentd in
  shadow mode before agent-chat is turned off.

## 2. Phase A — parity baseline and hard acceptance list

Purpose: stop guessing what "replace agent-chat" means.

Deliverables:

- `docs/parity/agent-chat-capability-map.md`: table of agent-chat capabilities,
  owner file, current agentd status, replacement decision, and cutover priority.
- `specs/e2e/p200-agent-chat-parity-baseline.spec.md`: acceptance contract for
  required replacement features.
- A small read-only audit command, for example
  `agentctl parity audit --agent-chat /path/to/agent-chat`, that prints missing
  required capabilities.

Required categories:

- Agent registry and lifecycle.
- Messaging: DM, group, inbox, mentions, unread/read semantics.
- Task and task-graph coordination.
- Pool scheduling: role x capability, reservations, dispatch/release.
- Runtime launch and tmux/session handling.
- Dashboard/operator visibility.
- CLI operations.
- Matrix bridge and remote relay.
- Migration/import/shadow mode.

Exit criteria:

- The parity map has no "unknown" rows for required agent-chat capabilities.
- Every future implementation phase links back to one or more parity rows.

## 3. Phase B — make real execution reliable, Codex-first

Purpose: agentd must first win at its core differentiator: typed workflow
execution.

Deliverables:

- Codex runtime launcher equivalent to the current Claude launcher path.
- Real-agent MCP config generation for Codex.
- Reviewer spawning that does not depend on Claude quota or Claude-specific
  assumptions.
- `scripts/agentd_real_execute_smoke.sh` supports a runtime matrix such as
  `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex`.
- Real execute evidence includes reviewer verdicts, aggregate result,
  publish/open-pr status, and terminal run snapshot.

Likely files:

- `crates/agentd-bin/src/host.rs`
- `crates/agentd-bin/src/agent_mcp_context.rs`
- `crates/agentd-bin/src/stdio_mcp.rs`
- `crates/agentd-tmux/src/*`
- `scripts/agentd_real_execute_smoke.sh`
- `specs/e2e/p200-agent-chat-parity-baseline.spec.md`

Exit criteria:

- Real execute smoke passes using Codex agents without requiring Claude.
  Achieved by `p204-codex-matrix-r9`; evidence:
  `.agentd/real-execute-smoke/p204-codex-matrix-r9/summary.txt`.
- A failed reviewer launch is reported as a structured run blocker, not a
  timeout that requires manual log inspection.

## 4. Phase C — agent registry and lifecycle parity

Purpose: replace agent-chat's agent inventory and local process control.

Update 2026-07-09: p213 lands the first registry lifecycle baseline: durable
agent records, `/api/agents` register/list/detail/heartbeat/offline endpoints,
and `agentctl agent` commands for the same surfaces. p214 adds launch-env,
Codex-testable start through `AgentBackend::spawn`, and minimal runtime
observation updates. p215 adds the first API auth boundary baseline: configured
bearer checks for operator routes, per-agent token checks with hard/audit modes,
and local-only launch-env/start protection. Phase C is still incomplete because
dashboard/browser auth, bridge and relay secrets, import-time token provisioning,
token rotation, kill/rebind/session recovery, and dashboard agent views are not
covered yet.

Deliverables:

- Durable `agents`, `agent_heartbeats`, `runtime_profiles`, and
  `agent_sessions` records.
- HTTP endpoints for register/list/detail/start/offline/heartbeat/runtime update.
- `agentctl agent ls`, `agentctl agent start`, `agentctl agent offline`,
  `agentctl agent inspect`.
- Backward-compatible identity model that can represent agent-chat agents:
  name, role, capability, runtime, tmux target, workdir, home dir, state dir,
  server, model, and status.

Likely files:

- `crates/agentd-store/migrations/*`
- `crates/agentd-store/src/*agent*`
- `crates/agentd-surface/src/http.rs`
- `crates/agentctl/src/cli.rs`
- `crates/agentctl/src/*`

Exit criteria:

- Existing agent-chat agent registry examples can be imported into agentd.
- Agent heartbeat/offline state is visible from CLI and dashboard.

## 5. Phase D — messaging and inbox parity

Purpose: replace agent-chat's MCP messaging surface.

Update 2026-07-09: p217 starts Phase D with a durable direct-message inbox
baseline: SQLite `direct_messages`, operator `POST /api/messages`, and
`check_inbox` preview/drain reads returning agent-facing `messages`/`dm`/`group`
fields. p218 adds direct `send_message` MCP writes through the same durable inbox,
and p219 binds stdio MCP sessions to an agent identity so `send_message` can omit
`from_agent`, `check_inbox` can omit `agent_id`, and spoofed sender/inbox access
is rejected before local or proxy dispatch. p220 adds durable groups, group
mentions, group history preview/read_all semantics, HTTP group endpoints, MCP
`post`, MCP `check_group`, and identity-bound stdio group tool injection. p221
adds the first attachments baseline: local readable-file validation and durable
agent-chat-style attachment metadata on direct and group messages. p222 adds
the first local media byte path: `/api/media/stage`, `/api/media/fetch`, daemon
media-directory persistence, and `staged=true` attachment metadata preservation
through `/api/messages`. p223 adds stdio MCP proxy media localization for
remote/isolated agents: proxied `check_inbox` and `check_group` responses fetch
staged media through `/api/media/fetch`, cache bytes on the agent side, rewrite
attachments and `LocalPath:` lines to readable local paths, reuse warm cache
files, and warn without failing when fetch fails. p224 adds non-destructive
message import and shadow audit for
`messages.json`, `groups.json`, and `cursors.json`, preserving direct/group
message history and read cursor state during migration. p225 adds
non-destructive task import, task-graph snapshot preservation, and shadow audit
for `tasks.json` and optional `task_graphs.json`, so migration can carry
product-level task coordination state without routing it into workflow
`task_runs`. p226 adds live `/api/tasks` CRUD parity over those compatibility
rows, including task comments, execution metadata, assignee task listing, bearer
operator writes, assignee token agent writes, and agent-chat lifecycle
transitions. p227 closes the live `/api/task-graphs` slice: graph create,
list, inspect, cancel, node update, task-graph DAG dispatch direct messages,
task-graph dispatch result handling, dependency advance, condition skip, and daemon
persistence now run on the compatibility table. p229 adds scheduled
role/capability task-graph nodes through the durable scheduler. Phase D remains
incomplete until dashboard message/task views, Matrix/remote relay state,
notification gates, service cutover, rollback, and token provisioning are
covered.

Deliverables:

- Durable messages with sender, recipient/group, body, summary, priority,
  reply-to, schema/type, attachments metadata, and created/read cursors.
- MCP tools compatible in spirit with agent-chat:
  `whoami`, `send_message`, `post`, `check_inbox`, `check_group`.
- HTTP endpoints for DM/group messages and read/unread state.
- Minimal media staging/fetch replacement or a documented local-file attachment
  rule. p221/p222/p223 now cover the local baseline and stdio proxy
  localization; Matrix and remote relay media remain separate parity slices.

Likely files:

- `crates/agentd-store/migrations/*`
- `crates/agentd-surface/src/mcp_server.rs`
- `crates/agentd-surface/src/tools/*`
- `crates/agentd-surface/src/http.rs`

Exit criteria:

- An agent-chat MCP client workflow can be ported to agentd without changing its
  collaboration model.
- `check_inbox` is no longer a v0 placeholder; it returns real unread messages.

## 6. Phase E — pool scheduler and dispatch parity

Purpose: replace agent-chat's matrix-agent execution layer.

Deliverables:

- Role and capability model: `architect`, `coding`, `testing`, `review`,
  `integration`, `documentation`; tiers `strong`, `medium`, `lightweight`.
- Durable reservations with dispatch/release semantics.
- Scheduler that selects an existing agent, queues if none is available, or
  returns a structured provision plan.
- Workflow integration so `codergen` and `fan_out` can request agents by
  role/capability instead of hardcoded reviewer identities.

Update 2026-07-09: p228 completes the pool scheduler baseline for the local
agent-chat replacement surface. Agentd now exposes `/api/pool` and
`/api/dispatch`, persists durable reservations, supports release draining,
records queue state, and returns structured provision plans without launching
real runtimes. p229 adds task-graph scheduler integration: scheduled nodes can
request role/capability, dispatched nodes expose reservation metadata, queued ticket drain
dispatches the next graph node, and result-time release frees the scheduler
reservation. p230 adds workflow scheduler allocation for DOT
`codergen` and `fan_out`: handlers request role/capability allocations through a
core port, persist selected agent ownership, expose scheduler metadata in
`run_parked` events, and release reservations when workflow work completes. The
p228 task-graph/workflow integration gap is now partly closed for task graphs
and workflow execution. Phase E remains incomplete until dashboard views,
Matrix/remote relay coverage, queued workflow wakeups, cutover, rollback, and
token provisioning are complete.

Update 2026-07-09: p231 adds existing-pane prompt reuse for routed online agents
selected by the workflow scheduler. The core backend seam now supports
allocation-aware dispatch, production scheduler allocations carry the registered
tmux target/runtime metadata, and the tmux backend uses tmux rebind plus
paste-buffer prompt injection so routed workflow dispatch performs no duplicate
spawn. This closes the online-agent prompt reuse gap without launching real
Claude, Matrix, systemd, launchd, or remote relay processes in tests.

Update 2026-07-09: p232 adds queued codergen workflow wakeup for the
scheduler-backed workflow path. When a `codergen` workflow allocation queues,
agentd now parks the original task run with one durable scheduler ticket and
stores the base prompt/worktree in checkpoint context. When a later scheduler
release drains that ticket, ProductionRunHost validates the parked run/node,
sets the task-run owner to the freed Codex agent, performs release-drain dispatch
through the allocation-aware backend, emits updated `run_parked`
scheduler metadata, and provides duplicate-dispatch suppression on replayed
completion.

Update 2026-07-09: p233 closes the remaining fan_out queued wakeups gap with
queued fan_out reviewer wakeup for scheduler-backed workflow review nodes. When a
`parallel.fan_out` reviewer allocation queues, agentd now parks the original
review run with one durable scheduler ticket and stores reviewer wakeup context
in the checkpoint. When a later reviewer release drains that ticket,
ProductionRunHost validates the parked review run/node, allocates or reuses a
review worktree for the freed Codex review agent, performs release-drain dispatch
through the allocation-aware backend, emits updated `run_parked` scheduler
metadata, and preserves duplicate-dispatch suppression on replayed reviewer
completion.

Update 2026-07-09: p234 adds the operator-local agent lifecycle baseline for
agent-chat replacement. Agentd now has `down` and `rebind` daemon/CLI surfaces,
uses a lifecycle port backed by tmux shutdown/rebind in real daemon assembly,
records runtime lifecycle metadata on agent rows, and supports explicit session
recovery from stored tmux targets after a daemon host rebuild. This keeps tests
Codex/fake-only and does not claim dashboard, Matrix/remote relay, cutover,
rollback, token provisioning, or full agent home/profile management parity.

Likely files:

- `crates/agentd-core/src/ports/backend.rs`
- `crates/agentd-bin/src/host.rs`
- `crates/agentd-store/src/*`
- `crates/agentd-surface/src/http.rs`
- `workflows/execute.dot`

Exit criteria:

- The 3-reviewer execute flow can request reviewer allocations through the
  scheduler-backed workflow allocator.
- Scheduler decisions are visible and auditable from run events.
- Routed online agents selected by workflow allocation receive prompts through
  existing-pane prompt reuse via tmux rebind and no duplicate spawn.
- Queued `codergen` workflow nodes wake after scheduler release drains a ticket
  and dispatch exactly once to the freed online agent.
- Queued `fan_out` reviewer workflow nodes wake after scheduler release drains a
  ticket and dispatch exactly once to the freed online review agent.
- Agent lifecycle `down` and `rebind` can stop a registered local runtime,
  recover a stored tmux session, and persist runtime lifecycle metadata for
  operator inspection.
- Remaining scheduler execution gaps are now outside queued workflow wakeups:
  dashboard visibility, Matrix/remote relay coverage, cutover, rollback,
  notification gates, token provisioning, and full agent home/profile
  management.

## 7. Phase F — dashboard and CLI operator parity

Purpose: make agentd usable without raw curl/log inspection.

Deliverables:

- Dashboard sections for runs, agents, queue/reservations, messages, blockers,
  and recent events.
- CLI commands covering common agent-chat operations: up/start, down/offline,
  ls, send/post, run start, parity audit, service status.
- Structured blocker reports for missing tools, auth, reviewer launch failure,
  PR history mismatch, and verification failure.

Likely files:

- `crates/agentd-surface/src/dashboard.html`
- `crates/agentd-surface/src/http.rs`
- `crates/agentctl/src/*`

Exit criteria:

- Daily local operation does not require opening raw SQLite, tmux panes, or smoke
  evidence files for normal status checks.

## 8. Phase G — Matrix bridge and remote relay

Purpose: cover the optional but important distributed coordination surfaces.

Update 2026-07-09: p235 adds the first backend-facing remote relay compatibility
baseline. Agentd now has durable server heartbeat state, `/api/servers/heartbeat`,
delivery-event audit through `/api/delivery-events` and
`/api/agents/:name/delivery-events`, and an agent-chat-compatible `/api/stream`
message wakeup stream. This makes `remote_relay` partial, not missing. Before
p236, Matrix bridge remained missing. Remote relay replacement is still
incomplete until a real remote package, install verification, relay process
operation, tmux injection, Matrix bridge integration, service cutover, rollback,
token provisioning/rotation, and dashboard relay views are complete.

Update 2026-07-09: p236 adds the backend-facing Matrix external bridge contract:
durable room trust and room mapping state, Matrix inbound ingress through
`POST /api/matrix/inbound`, Matrix outbox polling through
`GET /api/matrix/outbox`, event idempotency, and `[AGENTIGNORE]` suppression.
Matrix bridge is partial, not covered: agentd still needs a real Matrix bridge process, puppet accounts, room join/invite handling, Matrix media, bot commands, service cutover, rollback, token provisioning/rotation, and bridge operations before it can replace agent-chat Matrix usage.

Update 2026-07-09: p237 adds the `agentd-matrix` bridge runtime scaffold on top
of the p236 backend contract. The scaffold defines backend and Matrix transport
traits, a deterministic `run_once` loop for room registration, inbound event
forwarding, outbox polling, outbound sends, and cursor advancement/retry
semantics. It is covered with fake backend and fake Matrix transport tests, not
a real homeserver. Matrix bridge remains partial: agentd still needs a real
Matrix SDK process, puppet accounts, join/invite handling, Matrix media, bot
commands, service cutover, rollback, token provisioning/rotation, bridge
operations, and dashboard/operator visibility before it can fully replace
agent-chat Matrix usage.

Update 2026-07-09: p238 connects the `agentd-matrix` scaffold to agentd's p236
HTTP contract with `AgentdHttpBackend`. The client uses standard-library HTTP,
supports operator bearer auth, posts Matrix room and inbound JSON to the p236
field names, polls `/api/matrix/outbox?from_seq=N`, preserves outbox payload
metadata for later routing, and adds JSON cursor state persistence so confirmed
outbox progress can survive restart. Matrix bridge remains partial: agentd still
needs a real Matrix SDK process, homeserver login, puppet accounts, join/invite
handling, Matrix media, bot commands, service cutover, rollback, token
provisioning/rotation, bridge operations, and dashboard/operator visibility
before it can fully replace agent-chat Matrix usage.

Update 2026-07-09: p239 adds the first runnable one-shot bridge shell:
`matrix-bridge-once` composes `AgentdHttpBackend`, file-backed Matrix transport
fixtures, target-to-room resolution for trusted room mappings, JSONL sent-message
logging, and JSON cursor persistence. This makes the HTTP backend/runtime/state
path locally replayable without real Matrix or Claude, but Matrix bridge remains
partial: agentd still needs a real Matrix SDK process, homeserver login, puppet
accounts, join/invite handling, Matrix media, bot commands, long-running service
packaging, service cutover, rollback, token provisioning/rotation, bridge
operations, and dashboard/operator visibility before it can fully replace
agent-chat Matrix usage.

Update 2026-07-09: p240 adds the SDK-facing Matrix client adapter boundary in
`agentd-matrix`. `MatrixClientPort` and `MatrixClientBridgeTransport` now define
the normalized client operations a real Matrix SDK adapter must provide:
login-before-sync, trust-mode invite handling, single-snapshot room/message
sync, inbound loop suppression for bot/agent/ignored/`[AGENTIGNORE]` events, and
outbound text sends through the existing target-to-room directory. Matrix bridge
remains partial: this is still a fake-client-tested boundary, not a real Matrix
SDK process, homeserver login, puppet account lifecycle, media transfer, bot
command surface, service packaging, cutover, rollback, token provisioning, or
operator dashboard.

Update 2026-07-09: p241 adds a feature-gated real Matrix SDK adapter path:
`matrix-sdk-adapter` stays disabled by default, while `SdkMatrixClient` can be
built with the real `matrix-sdk` dependency and exposes password login,
access-token session restore, SDK `/sync`, room join, room leave, and plain-text
send operations through `MatrixClientPort`. This proves the SDK dependency path
compiles without connecting tests to a real homeserver. Matrix bridge remains
partial: agentd still needs puppet accounts, full timeline parsing, room
lifecycle parity, Matrix media, bot commands, service packaging, cutover,
rollback, token provisioning/rotation, bridge operations, and
dashboard/operator visibility before it can fully replace agent-chat Matrix
usage.

Update 2026-07-09: p242 adds SDK timeline text parsing for the real SDK adapter.
`SdkMatrixClient::sync_once` now consumes joined room timeline events from the
SDK `/sync` response, parses original `m.room.message` events into
`MatrixClientTextMessage`, preserves Matrix mention user ids and direct reply
targets, and skips state, redacted, non-room-message, and malformed events.
Matrix bridge remains partial: p242 covers text event normalization only, while
agentd still needs puppet accounts, room lifecycle parity, Matrix media, bot
commands, service packaging, cutover, rollback, token provisioning/rotation,
bridge operations, and dashboard/operator visibility before it can fully
replace agent-chat Matrix usage.

Update 2026-07-09: p243 adds puppet identity mapping for the Matrix bridge.
`MatrixPuppetDirectory` and `MatrixPuppetAccount` now provide a deterministic
local plan for known agent Matrix MXIDs, using explicit `server_name` and
`agent_user_prefix`, de-duplicating known agents case-insensitively, excluding
skipped service agents, and resolving only configured local puppet MXIDs back to
agent names. `MatrixClientBridgeTransport` can use this plan to suppress known
agent puppet loop messages without hiding arbitrary same-prefix Matrix users,
while retaining the prefix-only fallback for configs that do not yet provide a
server name. Matrix bridge remains partial: p243 covers puppet identity mapping
only, while account registration, password/token provisioning, room lifecycle
parity, Matrix media, bot commands, service packaging, cutover, rollback,
bridge operations, and dashboard/operator visibility remain before full
agent-chat replacement.

Update 2026-07-09: p244 adds a local puppet account provisioning plan for the
Matrix bridge. `MatrixPuppetProvisioningConfig` now derives agent-chat-compatible
password candidate values from `MATRIX_AGENT_PASSWORD_SECRET` and optional
legacy template settings, preserving the existing agent-chat legacy replacement
order for migration compatibility. `MatrixPuppetTokenState` resolves existing
agent token names case-insensitively and reports stale token names, while
`MatrixPuppetProvisioningPlan` classifies each planned puppet as existing-token
reuse, login/register needed, or missing password. `MatrixPuppetRegistrationAuth`
chooses registration-token UIA auth, dummy UIA auth, or a deterministic
`BridgeError::InvalidConfig` without contacting a real homeserver. Matrix bridge
remains partial: p244 covers local provisioning decisions only, while real account registration,
token persistence, whoami validation, display-name/avatar sync, room lifecycle
parity, Matrix media, bot commands, service packaging, cutover, rollback, token
provisioning, token rotation, bridge operations, and dashboard/operator
visibility remain before full agent-chat replacement.

Update 2026-07-09: p245 adds the puppet account executor boundary for the Matrix
bridge. `MatrixPuppetAccountExecutor` now executes the p244 local provisioning
decisions through `MatrixPuppetAccountPort` and `MatrixPuppetTokenSink`, so fake
tests cover existing-token whoami validation, password login candidate order,
registration after login failures, token persistence, stale token pruning, and
per-agent outcomes without touching a real homeserver. Matrix bridge remains
partial: p245 covers fake-tested account execution only, while real Matrix HTTP account registration,
SDK/account wiring, UIA probe execution, display-name/avatar
sync, room lifecycle parity, Matrix media, bot commands, service packaging,
cutover, rollback, token provisioning, token rotation, bridge operations, and
dashboard/operator visibility remain before full agent-chat replacement.

Update 2026-07-09: p246 adds the Matrix puppet HTTP account port. `MatrixPuppetHttpAccountPort`
implements the p245 `MatrixPuppetAccountPort` with standard-library HTTP and
agent-chat-compatible Matrix Client-Server request shapes for whoami, password
login, registration probe, registration-token UIA completion, and dummy UIA
completion. Tests use only local fake homeservers, including Matrix's common
401 UIA probe response, and do not connect to a real Matrix homeserver. Matrix
bridge remains partial: p246 covers the HTTP account port only, while SDK/daemon
assembly, durable token-store backends, display-name/avatar sync, room lifecycle
parity, Matrix media, bot commands, service packaging, cutover, rollback, token
provisioning, token rotation, bridge operations, and dashboard/operator
visibility remain before full agent-chat replacement.

Update 2026-07-09: p247 adds the Matrix puppet HTTP account provisioner
assembly. `MatrixPuppetHttpAccountProvisioner` constructs the p246
`MatrixPuppetHttpAccountPort` and delegates to the p245
`MatrixPuppetAccountExecutor`, so reusable token validation, login/register
fallback, registration-token UIA, token saves, stale-token pruning, and
per-agent failure continuation now run together through one library entry point.
Tests use local fake homeservers and in-memory token sinks only. Matrix bridge
remains partial: p247 covers library-level HTTP account provisioning assembly,
while daemon/SDK account provisioning assembly, real account registration
against an operator homeserver, real Matrix HTTP account registration assembly,
durable token-store backends, display-name/avatar sync, room lifecycle parity,
Matrix media, bot commands, service packaging, cutover, rollback, token
provisioning, token rotation, bridge operations, and dashboard/operator
visibility remain before full agent-chat replacement.

Update 2026-07-09: p248 adds the Matrix puppet token file store.
`MatrixPuppetTokenFileStore` reads agent-chat-style bridge-state JSON
`agentTokens` into `MatrixPuppetTokenState` and implements
`MatrixPuppetTokenSink` for saving new puppet tokens and deleting stale token
names. The store preserves unknown top-level bridge-state fields, replaces
existing token keys case-insensitively, creates first-run parent directories,
and writes through a temp-file-then-rename path. Tests cover first-run creation,
unknown-field preservation, malformed JSON protection, stale-token deletion, and
an HTTP provisioner run that persists a new token while pruning stale state.
Matrix bridge remains partial: p248 covers the file-backed bridge-state token
store only, while daemon/SDK account provisioning assembly, real account
registration against an operator homeserver, real Matrix HTTP account
registration assembly, additional durable token-store backends beyond the file
bridge-state store, display-name/avatar sync, room lifecycle parity, Matrix
media, bot commands, service packaging, cutover, rollback, token provisioning,
token rotation, bridge operations, and dashboard/operator visibility remain
before full agent-chat replacement.

Update 2026-07-09: p249 wires Matrix puppet account provisioning into the
one-shot bridge assembly. `BridgeOncePuppetAccountConfig` lets
`run_bridge_once` execute `MatrixPuppetHttpAccountProvisioner` with
`MatrixPuppetTokenFileStore` before the normal backend/file-transport runtime
pass, and `BridgeOnceReport` retains the puppet provisioning report when the
optional config is present. `agentd matrix-bridge-once` now accepts explicit
Matrix homeserver, server-name, agent, skip-agent, token-state, password,
legacy-template, and registration-token options, while incomplete configs fail
with `BridgeError::InvalidConfig` before bridge HTTP side effects. Matrix bridge
remains partial: p249 covers opt-in one-shot assembly only, while daemon/SDK
account provisioning assembly, daemon/SDK service assembly, real operator
homeserver validation, additional durable token-store backends beyond the file
bridge-state store, display-name/avatar sync, room lifecycle parity, Matrix
media, bot commands, service packaging, cutover, rollback, token provisioning,
token rotation, bridge operations, and dashboard/operator visibility remain
before full agent-chat replacement.

Update 2026-07-09: p250 adds the SDK-facing Matrix bridge one-shot assembly.
`MatrixClientBridgeOnceConfig` and `run_matrix_client_bridge_once` compose
`AgentdHttpBackend`, `MatrixClientBridgeTransport`, `BridgeRuntime`, JSON
cursor persistence, and optional p249 puppet provisioning around any
`MatrixClientPort`, so fake clients can verify the SDK-facing bridge path
without enabling the real SDK feature by default. Tests cover Matrix client
login/sync, HTTP backend room/inbound/outbox calls, outbound Matrix sends,
puppet provisioning before Matrix sync, and preserving the on-disk cursor when
Matrix send fails. Matrix bridge remains partial: p250 covers SDK-facing
one-shot assembly only, while daemon/SDK account provisioning assembly, daemon service assembly,
daemon/SDK service assembly, real operator homeserver
validation, additional durable token-store backends beyond the file
bridge-state store, display-name/avatar sync, room lifecycle parity, Matrix
media, bot commands, service packaging, cutover, rollback, token provisioning,
token rotation, bridge operations, and dashboard/operator visibility remain
before full agent-chat replacement.

Update 2026-07-09: p251 adds the daemon-side bounded Matrix client bridge
service assembly. `MatrixClientBridgeServiceConfig`,
`MatrixClientBridgeServiceReport`, and `run_matrix_client_bridge_service` live
in `agentd-bin::matrix_bridge` and run a positive bounded number of SDK-facing
bridge iterations through one injected mutable `MatrixClientPort`, reusing the
p250 one-shot path for HTTP backend calls, Matrix client sync/send behavior,
JSON cursor persistence, and optional p249 puppet account provisioning. The
new `agentd matrix-client-bridge-service` configuration surface captures
agentd API URL, state path, bounded iteration count, SDK credentials, transport
trust settings, known/skip agents, ignored senders, trusted inviters, and
optional puppet provisioning. Default builds remain SDK-free; the real
`SdkMatrixClient` entrypoint is feature-gated behind `agentd-bin`'s
`matrix-sdk-adapter` feature and tests use fake clients only. Matrix bridge
remains partial: p251 covers bounded service assembly but not
real homeserver validation, unbounded daemon supervision, service packaging, Matrix media, bot
commands, cutover, rollback, token provisioning, token rotation, bridge
operations, or dashboard/operator visibility.

Update 2026-07-09: p252 adds a Matrix client bridge operator preflight.
`MatrixClientBridgePreflightReport`, `MatrixHomeserverPreflightReport`, and
`run_matrix_client_bridge_preflight` live in `agentd-bin::matrix_bridge` and
reuse the p251 service option surface/config validation before any bridge run.
The new `agentd matrix-client-bridge-preflight` command probes
`/_matrix/client/versions`, optionally probes
`/_matrix/client/v3/account/whoami` with an access token, reports advertised
versions and the validated Matrix user id, and avoids cursor or puppet token
state mutation. Tests use only fake homeservers. Matrix bridge remains partial:
p252 covers fake-tested operator preflight but not a real operator homeserver
smoke, unbounded daemon supervision, service packaging, Matrix media, bot
commands, cutover, rollback, token provisioning, token rotation, bridge
operations, or dashboard/operator visibility.

Update 2026-07-09: p253 adds an explicit real-environment Matrix preflight
smoke harness. `scripts/agentd_matrix_client_bridge_preflight_smoke.sh`
supports dry-run, local preflight-only validation, and an execute mode that
requires `AGENTD_REAL_MATRIX_PREFLIGHT_SMOKE=1` plus a Matrix homeserver URL.
The execute path runs `agentd matrix-client-bridge-preflight`, captures
`preflight.out`, `preflight.err`, and `summary.txt`, redacts access-token
values in dry-run/summary output, and fails if the preflight creates the bridge
cursor state file. Default tests use fake binaries only and do not contact a
real homeserver. Matrix bridge remains partial: p253 covers the opt-in smoke
harness but not real CI execution against a homeserver, account registration,
unbounded daemon supervision, service packaging, Matrix media, bot commands,
cutover, rollback, token provisioning, token rotation, bridge operations, or
dashboard/operator visibility.

Update 2026-07-09: p254 adds an explicit bounded Matrix client bridge service
smoke harness. `scripts/agentd_matrix_client_bridge_service_smoke.sh` supports
dry-run, local preflight-only validation, and an execute mode that requires
`AGENTD_REAL_MATRIX_SERVICE_SMOKE=1`, a Matrix homeserver URL, and exactly one
Matrix SDK login mode: username/password or user-id/access-token. The execute
path builds `agentd-bin` with `--features matrix-sdk-adapter` unless
`--skip-build` is passed, runs `agentd matrix-client-bridge-preflight` before
`agentd matrix-client-bridge-service`, captures `preflight.out`,
`preflight.err`, `service.out`, `service.err`, and `summary.txt`, redacts
Matrix password/access-token/puppet-secret/registration-token values in
dry-run and summary output, and verifies that the bounded service command
created the bridge cursor state file. Default tests use fake binaries only,
do not start a real daemon, and do not contact a real homeserver. Matrix bridge
remains partial: p254 covers the opt-in bounded service smoke harness but not
real CI execution against a homeserver and daemon, account registration
evidence, unbounded daemon supervision, service packaging, Matrix media, bot
commands, cutover, rollback, token provisioning, token rotation, bridge
operations, or dashboard/operator visibility.

Update 2026-07-09: p255 adds an `agentd-matrix` bot command planner for the
agent-chat Matrix command grammar. It recognizes bang commands, strips Matrix
mention-pill command prefixes, classifies the agent-chat command set into
public, operator read, operator management, and admin tiers, evaluates Matrix
operator/admin ACLs with the same empty-ACL compatibility behavior, preserves
room context for group/agent DMs, and returns the agent-chat fallback hint
`Send !help for available commands.` for non-command bot-DM messages. Matrix
bridge remains partial: p255 covers bot command planner compatibility but not
command execution, backend mutation, Matrix sends, room lifecycle changes,
tmux/agentctl control, service packaging, Matrix media, cutover, rollback,
token provisioning, token rotation, bridge operations, or dashboard/operator
visibility.

Update 2026-07-09: p256 adds bot command ingress classification to the
SDK-facing Matrix client transport. `MatrixClientTextMessage` now preserves
optional `formatted_body` for Matrix mention-pill command detection, and
`MatrixClientBridgeTransport` applies loop and ignored-sender suppression before
planning command-shaped events. Command events are stored through
`bot_command_plans` and omitted from normal inbound forwarding, while normal
non-command group/agent-DM text still forwards to agentd. The
`agentd matrix-client-bridge-service` and `agentd matrix-client-bridge-preflight`
surfaces now accept repeatable `--matrix-operator` and `--matrix-admin` MXIDs so
the planner can use agent-chat-compatible command ACLs. This is command omission from inbound forwarding, not command execution. Matrix bridge remains partial:
p256 does not execute commands, send Matrix replies, mutate backend command
state, control tmux/agentctl, create rooms, transfer Matrix media, package the
service, cut over, roll back, rotate tokens, expose bridge operations, or add
dashboard/operator visibility.

Update 2026-07-10: p257 adds read-only bot command replies to the SDK-facing
Matrix client bridge path. `execute_matrix_bot_command` now renders
agent-chat-shaped plain-text replies for `!help`, `!status`, `!agents`,
`!agents all`, `!groups`, ACL denials, unknown commands, and explicit
unsupported-command notices without mutating backend state. `AgentdHttpBackend`
reads `/api/agents` and `/api/groups` into `MatrixBotCommandSnapshot`, and
`MatrixClientBridgeTransport` sends replies through the Matrix client while
keeping command events out of normal inbound forwarding. One-shot and bounded
service reports expose `bot_command_replies_sent`. Matrix bridge remains partial:
p257 does not execute management/admin commands such as `!dm`, `!identity`,
`!rmgroup`, `!spy`, `!agentctl`, or `!ctl`, does not create or modify Matrix
rooms, does not control tmux/agentctl, does not cover Matrix media, service
packaging, cutover, rollback, token rotation, bridge operations, or
dashboard/operator visibility.

Update 2026-07-10: p258 adds management command effects for `!dm` and
`!identity` without claiming full Matrix room lifecycle parity. The
`agentd-matrix` command executor now has explicit backend and room effect ports:
`!dm <agent>` verifies the target agent, requests a human-agent DM room for the
sender localpart, replies for invited/already-joined/invite-failed/no-room
outcomes, and declares `ChangesMatrixRooms`; `!identity [agent] <text>` resolves
the target from direct-room context or explicit args, requests an identity
update, replies with success or `Failed: ...`, and declares `MutatesBackend`.
`MatrixClientBridgeTransport` can execute command replies through these effect
ports while keeping command-shaped events out of normal inbound forwarding, and
`AgentdHttpBackend` exposes the request shape for `GET /api/agents/:name` plus
`PATCH /api/agents/:name` with an `identity` field. Matrix bridge remains
partial: p258 still does not implement full room creation/invite lifecycle in
the real SDK adapter, daemon-side identity persistence semantics, the remaining
management/admin commands, Matrix media, service packaging, cutover, rollback,
token rotation, bridge operations, or dashboard/operator visibility.

Update 2026-07-10: p259 adds daemon identity persistence for the p258
`!identity` path. `PATCH /api/agents/:name` now accepts an `identity` JSON
field through the daemon HTTP surface, stores the trimmed text in
`runtime_profile.identity`, preserves existing `runtime_profile` keys, rejects
empty identity text before mutation, returns `agent_not_found` for unknown
agents, and persists the update across production router rebuilds. Matrix
bridge remains partial: p259 still does not implement real SDK DM room
lifecycle, the remaining management/admin commands, Matrix media, service
packaging, cutover, rollback, token rotation, bridge operations, or
dashboard/operator visibility.

Update 2026-07-10: p260 adds SDK-facing DM room lifecycle for the p258 `!dm`
path. `!dm <agent>` room effects now use the sender's full Matrix MXID, derive
the agent puppet MXID through the existing `MatrixPuppetDirectory`, reuse a
trusted direct room for the target agent when one is present, inspect human
membership before sending duplicate invites, create a direct room named
`DM: <agent>` with both the human and agent puppet MXIDs when no trusted direct
room exists, and report invite failures while preserving the Matrix room link.
The real `SdkMatrixClient` implements these lifecycle operations behind the
existing `matrix-sdk-adapter` feature, while default tests stay on fake Matrix
clients and do not contact a real homeserver. Matrix bridge remains partial:
p260 still does not provide real homeserver operator evidence, per-agent puppet
client execution/token rotation evidence, the remaining management/admin
commands, Matrix media, service packaging, cutover, rollback, bridge
operations, or dashboard/operator visibility.

Update 2026-07-10: p261 adds Matrix bot group management effects for
`!mkgroup`, `!addmember`, `!rmember`, and `!rmgroup`. The Matrix command
executor now routes those authorized commands through backend effect ports:
group creation, explicit or group-room-context membership add/remove, group
lookup before removal, and durable group delete. `AgentdHttpBackend` maps the
effects to `POST /api/groups`, `POST /api/groups/:name/members`,
`GET /api/groups/:name`, and `DELETE /api/groups/:name`, and the daemon surface
now exposes `DELETE /api/groups/:name` backed by store-level group deletion and
SQLite cascade cleanup for members/messages. Matrix bridge remains partial:
p261 still does not implement `!joingroup`, admin commands (`!spy`,
`!agentctl`, `!ctl`), Matrix group-room leave/kick cleanup for `!rmgroup`, real
homeserver evidence, per-agent puppet client execution/token rotation evidence,
Matrix media, service packaging, cutover, rollback, bridge operations, or
dashboard/operator visibility.

Update 2026-07-10: p262 adds Matrix bot `!joingroup` effects. The command
adds the sender human localpart to the daemon group through the existing
backend member-update route and, when a trusted Matrix group-room mapping is
known, performs a trusted group-room invite for the sender's full Matrix MXID.
Missing room
mappings are reported without a homeserver call. Matrix bridge remains partial:
p262 still does not implement admin commands (`!spy`, `!agentctl`, or
`!ctl`), Matrix media, real homeserver evidence, service packaging, cutover,
rollback, token rotation, bridge operations, or dashboard/operator visibility.

Deliverables:

- A Matrix bridge adapter, or a documented decision that Matrix remains external
  and agentd exposes bridge-ready events/APIs.
- Remote relay strategy: either a first-party relay package or compatibility
  with the existing agent-chat remote relay during migration.
- Server/fleet heartbeats if multi-host operation is still a replacement
  requirement.

Likely files:

- `crates/agentd-matrix/src/lib.rs`
- new adapter crate if needed
- `docs/operations/*`

Exit criteria:

- Projects currently relying on agent-chat Matrix/remote paths have an agentd
  replacement or a consciously retained external dependency.

## 9. Phase H — migration, shadow mode, and cutover

Purpose: replace agent-chat without data loss or workflow interruption.

Deliverables:

- Import tool for agent-chat JSON state into agentd tables.
- Shadow mode where agentd observes or mirrors selected agent-chat events.
- Cutover checklist with rollback: stop agent-chat services, start agentd,
  verify agents/messages/runs, rollback to agent-chat if gates fail.
- Archived compatibility report for each migrated project.

Update 2026-07-09: p216 starts this phase with a narrow, non-destructive
`agents.json` import and `shadow-agents` audit for the registry schema that
already exists in this worktree. p224 extends migration coverage with
non-destructive `messages.json`, `groups.json`, and `cursors.json` import plus
`shadow-messages` audit, preserving direct/group message history and read cursor
state. p225 extends migration coverage with non-destructive `tasks.json` import,
optional `task_graphs.json` snapshot preservation, and `shadow-tasks` audit.
Phase H still does not cover live task CRUD, DAG dispatch, Matrix/remote relay
state, notification gates, dashboard message views, service cutover, rollback,
or token provisioning.

Likely files:

- `crates/agentctl/src/*migration*`
- `docs/operations/agent-chat-cutover.md`
- `scripts/*agent_chat*`

Exit criteria:

- One real project completes a full day of work on agentd with agent-chat off,
  while rollback remains possible.

## 10. Recommended execution order

1. Phase A: parity baseline.
2. Phase B: Codex-first real execution reliability.
3. Phase C: agent registry/lifecycle.
4. Phase D: messaging/inbox.
5. Phase E: pool scheduler.
6. Phase F: dashboard/CLI operations.
7. Phase G: Matrix/remote surfaces.
8. Phase H: migration/shadow/cutover.

Do not start with dashboard polish or Matrix. The replacement risk is currently
in execution reliability, registry/lifecycle semantics, messaging, and scheduler
parity. Those form the minimum product surface that agent-chat users actually
depend on.

## 11. Phase I — reconciled enterprise execution control plane

Purpose: continue from the verified P200-P262 base without merging the sibling
worktree's conflicting P202-P228 ids or migration versions. P263 records the
source-to-base mapping in
`docs/parity/agent-chat-worktree-reconciliation.md` and freezes this worktree as
the integration authority.

P264 resolves the enterprise source-of-truth boundary in
`docs/specs/2026-07-10-enterprise-execution-ownership-boundary.md` before any
enterprise identity or schema slice:

- **Specify Project Authority** (`SpecifyProjectAuthority`) owns project,
  repository, project-room binding, issue/spec lifecycle, product workflow,
  project RBAC/quota policy intent, and certification policy declarations.
- **Agentd Execution Control Plane** (`AgentdControlPlane`) owns durable worker,
  agent capability, queue/lease, runtime session, execution checkpoint,
  artifact index, audit, and measured usage state.
- `AgentdWorker` owns replaceable live process/PTY, worktree/cache, and upload
  spool state. `OpenFabCertificationAuthority` owns certification evidence,
  while `MatrixRobrixTransport` owns interaction and transport state.

These ownership labels are normative for P265-P279. Standalone and enterprise
modes use the same logical ports and identity model; configured Specify errors
fail closed rather than selecting a local project authority.

P265 freezes that shared identity model in
`docs/specs/2026-07-10-enterprise-runtime-worker-identity-contract.md`:
agent profiles, stable workers, worker incarnations, logical runtime sessions,
runtime attempts, execution runs/tasks, and leases have separate opaque ids.
Legacy agent ids, host names, Matrix ids, tmux targets, process ids, worktree
paths, provider resume refs, and dispatch tickets remain compatibility aliases,
locators, or metadata. P267 must implement these identities without collapsing
their lifecycle or fencing boundaries.

P266 refines `ProjectAuthorityPort` and `ProjectAuthorityRef` in
`docs/specs/2026-07-10-enterprise-project-room-repo-reference-contract.md`.
Every run pins one immutable authority snapshot containing versioned project,
repository/base commit, Matrix binding, frozen spec, workflow, RBAC, quota, and
certification policy inputs. Agentd stores references and execution evidence;
it does not own those project resources or infer them from legacy paths/rooms.

P267 implements the first enterprise agent/worker schema slice in
`0013_enterprise_agent_worker_runtime.sql`. Five typed ids, four lifecycle
enums, and additive repositories now cover agent profiles/legacy aliases,
stable workers/current incarnations, and logical runtime sessions/current
attempts. This is storage evidence only: worker networking, control-plane APIs,
scheduler leases, native runtime execution, and compatibility cutover remain
later slices.

P268 implements the immutable enterprise artifact/audit store slice in
`0014_enterprise_artifact_audit.sql`. Typed execution-artifact and audit-event
ids, immutable artifact provenance, explicit legacy artifact mappings,
append-only OpenFab certification references, and database-ordered idempotent
audit replay now have additive repositories and backcompat coverage. This is
still storage evidence only: API/upload/certification integration, object
storage, policy enforcement, dual-write, and compatibility cutover remain
later slices.

P269 implements the control-plane project-authority API boundary. The core
`ProjectAuthorityPort` now carries complete immutable P266 snapshots through
`resolve`, exact `refresh`, and diagnostic `health` calls. The separate
`agentd-project-authority` crate provides explicit `LocalProjectAuthority`, a
transport-injected `SpecifyProjectAuthority`, and new-execution pin plus
live/bounded-offline recovery decisions. This remains API evidence only:
Specify network transport, durable run pinning, legacy mappings, Matrix command
normalization, and cutover remain later slices.

P270 implements the control-plane dispatch, lease, and fencing API. Core now
defines canonical `LeaseId`, non-zero task-scoped `FencingToken`, closed lease
states, typed claim/rejection values, and `TaskLeasePort` dispatch, renewal,
release, cancellation, validation, and expiry operations. Migration
`0015_enterprise_task_leases.sql` adds immutable lease history plus durable
per-task token heads, while `SqliteTaskLeaseControlPlane` serializes grant
allocation with `BEGIN IMMEDIATE`, rejects stale or superseded claims, and
never promotes compatibility scheduler tickets/reservations into lease
identity. This remains API and storage evidence: P271 audit integration, P278
authenticated worker pull/recovery, scheduler/runtime integration, dual-write,
compatibility cutover, and rollback remain pending.

P271 implements the control-plane execution-evidence API without a schema
change. Core now defines `ArtifactIndexPort`, `ExecutionAuditPort`,
`UsageLedgerPort`, and `CertificationReferencePort` with bounded stable
cursors. `SqliteExecutionEvidenceControlPlane` exposes P268 artifact metadata,
audit replay, and external certification refs; records typed measured usage as
`usage.measured` audit events; and requires P270 lease claims for worker
artifact/usage reports. Stale, terminal, expired, and superseded reports append
`execution.report_rejected` before returning the typed rejection. Object
storage, OpenFab network transport, authenticated worker transport, policy
enforcement, dual-write, cutover, and delivery gating remain pending.

Planned spec sequence:

- P263 — worktree namespace, capability, and migration reconciliation.
- P264 — SpecifyProjectAuthority, AgentdControlPlane, AgentdWorker,
  OpenFabCertificationAuthority, and MatrixRobrixTransport ownership boundary.
- P265 — enterprise agent-profile, worker/incarnation, runtime-session/attempt,
  run/task, and lease identity contract.
- P266 — immutable project-room-repository/spec/policy authority references.
- P267 — enterprise agent/worker/runtime additive store model.
- P268 — immutable execution artifact and append-only audit store model.
- P269 — control-plane ProjectAuthorityPort API and standalone/Specify adapters.
- P270 — control-plane dispatch, lease, and fencing API.
- P271 — control-plane artifact, audit, usage, and certification-reference API.
- P272 — runtime status/capture/shutdown/rebind compatibility port.
- P273 — scheduler provision-registration reconciliation and Codex auto-spawn.
- P274 — relay unread backfill, push-delivered acknowledgement, and delivery lookup.
- P275 — durable direct-message suppression.
- P276 — agentd-owned native PTY/process runtime.
- P277 — native logical session persistence and provider resume.
- P278 — authenticated worker fleet pull protocol and durable lease recovery.
- P279 — enterprise RBAC/quota enforcement plus operator doctor diagnostics.

Migration authority follows the base chain: P267 uses migration `0013` after
`0012_matrix_bridge_contract.sql`; P268 uses migration `0014` after P267.
P270 uses additive migration `0015_enterprise_task_leases.sql` after P268 and
does not alter compatibility scheduler or task rows.
P271 reuses the P268/P270 schema and adds no migration; measured usage remains
a typed append-only audit event rather than a parallel identity/table.
Sibling migration SQL is design evidence only and must be adapted to current
base tables with fresh and backcompat tests.

Phase I does not erase Phase C-H compatibility work. The P263 manifest leaves
P205, P210, P211, P219, and P220 source behavior in an explicit port queue
represented by P272-P275. Enterprise contracts cannot be used to mark those
behaviors covered.

## Immediate Next Step

P263 reconciled the two same-HEAD dirty worktrees and reserved a collision-free
base sequence. P264 ownership now names exactly one authority for project,
execution, worker-local runtime, certification, and Matrix transport state
without changing production code. P265 now defines the durable runtime/worker
identity and fencing contract. P266 now defines the foreign authority reference model for project, repository, Matrix binding, frozen spec, workflow, policy, and quota snapshots. P266 MUST NOT add agentd-owned project tables.
P267 is the first enterprise agent/worker schema slice using migration
`0013_enterprise_agent_worker_runtime.sql`. P268 now implements the
artifact/audit model from `p268-enterprise-artifact-audit-model.spec.md` using
`0014_enterprise_artifact_audit.sql`, preserving immutable execution artifacts,
append-only audit ordering, and external OpenFab references. P269 now
implements the control-plane project API in
`p269-control-plane-project-api.spec.md` with `ProjectAuthorityPort`,
explicit `LocalProjectAuthority`, fail-closed `SpecifyProjectAuthority`, and
control-plane snapshot pin/recovery decisions. P270 now implements
`p270-control-plane-dispatch-api.spec.md` with `TaskLeasePort`, migration
`0015_enterprise_task_leases.sql`, durable task-scoped fencing, exact claim
validation, and serialized dispatch without reusing scheduler tickets. P271
now implements `p271-control-plane-artifact-audit-api.spec.md` with
`ArtifactIndexPort`, `ExecutionAuditPort`, `UsageLedgerPort`,
`CertificationReferencePort`, typed `usage.measured` events, and fenced worker
rejection audit. Next implement `p272-runtime-compatibility-port.spec.md` as
the runtime status, capture, shutdown, and rebind compatibility port.
