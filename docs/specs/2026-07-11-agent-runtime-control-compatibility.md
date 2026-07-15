# P272 Agent Runtime Control Compatibility Design

Status: design approved; implementation paused as FSF-0 transitional parity
work. P263-P271 now exist as reviewable feature-branch commits and the FSF/AD-E
declarations are in place, but those prerequisites do not authorize execution.
Resume requires an explicit human scope decision under the canonical roadmap.

## Context

The P263 reconciliation maps sibling P205 runtime status, capture, shutdown,
and rebind behavior to base P272. The base already has the underlying
`TmuxBackend` operations and P234 operator `down`/`rebind` routes, but it does
not expose status, capture, or an explicit archive-path shutdown through the
registered-agent API. P272 ports that capability without changing the
spawn-only `AgentBackend` contract or introducing a new runtime schema.

## Chosen Architecture

Evolve the P234 composition-root `AgentLifecycle` trait into
`AgentRuntimeControl`. The port owns four operations over an `AgentHandle`:
status, capture, archive-first shutdown, and rebind. `TmuxRuntimeControl`
adapts the existing `TmuxBackend` methods in production; tests inject a
recording fake. `AgentBackend` remains responsible only for spawn and allocated
prompt dispatch.

This is preferred over a parallel status/capture port because parallel ports
would duplicate shutdown and rebind state transitions. Moving the control port
into `agentd-core` is also rejected: P272 is a compatibility surface. The old
P276/P277 labels map to native runtime work under AD-E5 after the AD-E1 and
AD-E2 gates.

## API Surface

The existing `/api/agents` namespace gains:

- `GET /api/agents/:name/status`
- `GET /api/agents/:name/capture?lines=200&ansi=false`
- `POST /api/agents/:name/shutdown` with `archive_to` and optional `reason`

The existing `POST /api/agents/:name/rebind` accepts an optional `target`.
When omitted it keeps the P234 behavior of using the stored `tmux_target`.
The existing `POST /api/agents/:name/down` remains an operator convenience
that chooses the archive path itself and uses the same runtime-control port.

Status and capture require the configured operator bearer boundary. Shutdown,
down, and rebind retain the local-operator restriction because they mutate or
control a local process. Unknown agents return 404. Missing stored handle data,
an empty shutdown archive path, or an empty rebind target returns 400. Backend
failures remain 500.

## Runtime State

P272 reconstructs a tmux `AgentHandle` from the current agent row. The
canonical compatibility target is the non-empty `tmux_target`; the session
name is the target prefix before the first colon. No tmux pane, PID, path, or
provider session value becomes a P265/P267 enterprise identity.

Status maps the existing `AgentStatus` variants to stable wire values:
`gone`, `unexpected_shell`, `idle`, `busy`, and `starting`. Idle/busy include
`last_output_age_ms`; unexpected shell includes a detail string. A `gone`
probe records lifecycle state and marks the agent offline with
`runtime-gone`, clearing the dead target. Other statuses preserve the target,
mark the registry row online, and record the observation under
`runtime_state.lifecycle`.

Capture is read-only and returns the captured text plus the effective lines and
ANSI flag. Omitted query values are 200 and false. The request uses `u32`, so
negative or overflowing values are rejected by query deserialization.

Explicit shutdown requires a non-empty archive path. The runtime-control
adapter performs the existing tmux archive-before-kill sequence, after which
the host records method/path/SHA, marks the agent offline, and clears the dead
target. The default reason is `runtime-shutdown:<method>`.

Rebind with an explicit target may recover a target after an offline record has
cleared the old one. A live result updates `tmux_target`, online state, and
lifecycle metadata. A missing runtime preserves P234 compatibility: HTTP 200
with `rebound=false`, an offline row, and reason `rebind-missing-session`, not
the sibling draft's conflicting HTTP 409.

## CLI

`agentctl agent` gains `status`, `capture`, and `shutdown`; `rebind` gains an
optional `--target`. All use the operator bearer token path and print the
daemon response unchanged on success. Runtime-control commands reject an empty
name or a name containing `/` with exit code 2 before opening a socket.
Transport or daemon rejection remains exit code 3.

## Boundaries

P272 adds no dependency, migration, table, worker protocol, native PTY,
provider-resume behavior, scheduler reconciliation, remote lifecycle control,
dashboard UI, Matrix behavior, dual-write, cutover, or rollback. It does not
start real tmux, Claude, Codex, Matrix, or remote services in tests.

If resumed, P272 closes only the reconciled sibling P205 compatibility
capability. P272-P275 remain paused FSF-0 transitional parity candidates; P273
does not automatically follow this design.

## Verification

Tests use the existing surface fake host, a recording runtime-control port over
temporary SQLite, and one-shot fake HTTP daemons. They prove route defaults and
validation, exact port calls, registry state transitions, P234 down/rebind
compatibility, CLI request paths/bodies and pre-network validation, production
tmux adapter wiring by source inspection, and roadmap/parity advancement.
