# agentd — Control Plane vs Stream Plane (P1 architecture note)

> Status: descriptive (as-built across P0.9 + P1, tags `v0.0.0-p0.9` onward).
> Refines design doc [§1.3 Subsystem Responsibilities](specs/2026-05-29-agentd-design.md)
> (the `EventBus` / `HTTP+SSE` rows). Not a new requirement — a name for the
> structure the P1 borrows (live SSE tail, runs overview, bounded read, startup
> guard) made explicit.

## The distinction

agentd's daemon carries two kinds of traffic with **opposite** reliability
contracts. P0.9 and the P1 work keep them on separate planes so that a slow or
hostile observer can never stall the engine.

| | **Control plane** | **Stream plane** |
|---|---|---|
| Purpose | drive + persist run state | observe run progress live |
| Contract | **reliable, durable, authoritative** | **lossy, droppable, best-effort** |
| Backpressure | may block the caller (awaited writes) | **never** blocks the engine |
| On overload | n/a — every write is awaited + persisted | drop, then resync from the control plane |
| Source of truth | yes (`runs`, `events`, checkpoints) | no — a view, re-derivable from the control plane |

The design doc's §1.3 listed a single `EventBus` ("single broadcast channel for
all internal events … does NOT persist"). As built, that one row is two things:
the **durable** event log (control plane, the authority) and the **live**
broadcast (stream plane, the tail). The broadcast does not persist — the
`event_repo` log does — which is exactly why the broadcast is free to be lossy.

## What lives on each plane

**Control plane** (reliable; the engine + store):

- **Ingress** — `POST /runs` (start), and the MCP tools `submit_outcome` /
  `submit_review` (agent-process callbacks). Each is an awaited request that
  advances the engine.
- **Durable emit** — `event_repo::append` writes one row per state change and
  returns a monotonic `seq`. This is the audit log and the SSE **replay** source.
- **Authoritative reads** — `run_snapshot` (latest status + checkpoint) and
  `list_runs` (`GET /runs`, the fleet overview, P1 #2). These are queried, not
  streamed, and are always current.

**Stream plane** (lossy; observability only):

- **The live broadcast** — a single bounded `tokio::sync::broadcast`
  (capacity 256, `LIVE_BROADCAST_CAPACITY` in `agentd-bin/src/host.rs`). The emit
  point **dual-writes**: `event_repo::append` first (durable), then
  `live_tx.send` (live). `send` never blocks — an absent or slow subscriber
  never backpressures the engine.
- **The SSE tail** — `GET /runs/:id/events` (P1 #1): subscribe, replay from the
  `from_seq` cursor (control-plane read), then tail the broadcast, deduping the
  replay overlap by `seq`, until a terminal event.

## The invariant (the mosh / herdr lesson)

> A slow consumer of the stream plane must never stall the control plane.

This is the mosh / tmux-CC model: a reliable command/state path plus a droppable
high-frequency progress stream. agentd's refinement over herdr's literal
two-channel design is that agentd already has an **authoritative queryable
state** (`run_snapshot`) the control plane owns — so the stream plane needs only
**one** droppable channel, not two:

- **Normal:** the subscriber keeps up; each live event is forwarded once.
- **Lag** (`RecvError::Lagged`): the subscriber fell more than 256 events behind.
  Instead of backfilling every dropped event, the tail emits **one**
  `state_resync` frame built from `run_snapshot` — realign to the latest
  authoritative state, not replay history. (This is why the broadcast can be
  lossy at all: the control plane can always answer "where is this run now?")
- **Closed:** the run ended; the tail closes.

The same one-way dependency shows up at the edges hardened in P1:

- **#3 bounded read** (`agentctl` `read_response`): the client caps the bytes it
  buffers from the daemon — a stream-plane peer can't OOM a control-plane client.
- **#4 startup guard** (`bind_listener`): `bind` is the race-free authority for
  "is a daemon already here?"; the clear message is a control-plane concern,
  decided before any stream is opened.

## Why it matters

- **The engine is never held hostage by a dashboard.** Backpressure flows toward
  observers (they lag and resync), never toward the engine.
- **Recovery is a control-plane read, not a stream replay.** A client that
  reconnects after any gap calls `GET /runs` + `GET /runs/:id` (+ replay from a
  cursor) and is immediately correct — it does not need an unbroken event stream.
- **Durability is independent of liveness.** The audit log (`events`) is written
  and awaited regardless of whether anyone is subscribed; the live tail is pure
  convenience on top.

## Cross-references

- Live SSE tail + resync-on-lag — [`specs/surface/p75-sse-live-tail.spec.md`](../specs/surface/p75-sse-live-tail.spec.md)
- Runs overview (authoritative latest-state read) — [`specs/surface/p76-runs-overview.spec.md`](../specs/surface/p76-runs-overview.spec.md)
- Bounded client read — [`specs/workflow/p83-post-run-bounded-read.spec.md`](../specs/workflow/p83-post-run-bounded-read.spec.md)
- Startup guard — [`specs/e2e/p97-daemon-startup-guard.spec.md`](../specs/e2e/p97-daemon-startup-guard.spec.md)
- Emit dual-write — `agentd-bin/src/host.rs` (`ProductionRunHost::emit`)
- The tail state machine — `agentd-surface/src/http.rs` (`live_event_stream`)
