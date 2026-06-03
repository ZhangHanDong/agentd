spec: task
name: "Live SSE event tail with lossy-broadcast backpressure + snapshot resync"
tags: [surface, sse, p1, backpressure, herdr]
---

## Intent

P0.7's SSE endpoint is a FINITE replay (it streams the run's stored events once
and ends). This P1 task adds the live tail (borrowed from herdr/mosh): after
replaying history, the stream stays open and pushes new events as they are
emitted. A slow dashboard must NEVER backpressure the engine, so live events go
through a LOSSY, bounded `tokio::sync::broadcast`: the emit point's `send` never
blocks, and a lagging subscriber is realigned with a single authoritative
`run_snapshot` resync rather than backfilling every dropped event. The emit point
dual-writes (store = durable/audit; broadcast = live). agentd-core stays frozen
(D1); agentd-surface stays store-free (P0.7 D2) — the live stream is reached
through a `RunHost::subscribe_events` seam.

## Decisions

- A `LiveEvent { run_id, event: EventRecord }` is broadcast on one bounded `tokio::sync::broadcast` channel held by the host; `RunHost::subscribe_events()` returns a `broadcast::Receiver<LiveEvent>` (the surface filters by `run_id`).
- The emit point DUAL-WRITES: `event_repo::append` (durable) AND `broadcast.send(LiveEvent{..})` (live). `send` is non-blocking and lossy by design, so a slow/absent subscriber never blocks the engine loop.
- The SSE handler subscribes BEFORE replaying, replays `events_from(from_seq)`, then tails the broadcast — sending only live events whose `seq` is greater than the max replayed `seq` (dedup the overlap window; no gap because the subscription precedes the replay read).
- On `RecvError::Lagged`, the handler emits ONE `run_snapshot` resync frame (event `state_resync`) instead of erroring or backfilling — the lagging dashboard realigns to the latest authoritative state.
- A terminal event (`run_finished` / `run_failed`) closes the live stream. Under lag where the terminal frame was dropped, the resync snapshot's terminal status still closes it.

## Boundaries

### Allowed Changes

- crates/agentd-surface/**
- crates/agentd-bin/**
- specs/surface/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not pull agentd-store into agentd-surface (P0.7 D2 — the broadcast is reached via the RunHost seam; the store dual-write is the agentd-bin impl).
- Do not let the emit point block the engine on a slow subscriber (the broadcast send must be lossy/non-blocking).

## Out of Scope

- Per-run broadcast channels (one global lossy channel filtered by run_id is the MVP; per-run isolation so a noisy run cannot lag a quiet-run watcher is a later refinement).
- Broadcast-sender lifecycle/cleanup when no subscribers remain.

## Completion Criteria

Scenario: the emit point both persists and broadcasts an event
  Test: emit_persists_and_broadcasts
  Given a production host with a live subscription taken before the run starts
  When a draft.dot run is started (emitting run_parked)
  Then events_from returns the persisted event AND the subscriber receives the same live event

Scenario: the live stream replays history then tails new events without duplication
  Test: live_stream_replays_then_tails_without_dup
  Given a replay of events up to seq 2 and a subscription that then receives seq 2 and seq 3
  When the live event stream is collected
  Then it yields the replayed frames then only the seq-3 live frame (the seq-2 overlap is deduped)

Scenario: a lagging subscriber gets a snapshot resync instead of an error
  Test: live_stream_lag_sends_snapshot_resync
  Given a subscriber whose broadcast receiver has lagged past its buffer
  When the live event stream is collected
  Then it emits a "state_resync" frame carrying the run snapshot rather than failing

Scenario: a terminal event closes the live stream
  Test: live_stream_terminal_closes
  Given a subscription that receives a run_finished event
  When the live event stream is collected
  Then the stream yields the run_finished frame and then ends

Scenario: a lagging subscriber whose terminal was evicted closes via the snapshot status
  Test: live_stream_lag_with_terminal_snapshot_closes_via_resync
  Given a lagging receiver whose buffer holds only non-terminal events and a run_snapshot whose status is "finished"
  When the live event stream is collected
  Then it emits a resync frame and ends via the snapshot's terminal status (no terminal event was present)

Scenario: events for another run are filtered out
  Test: live_stream_filters_by_run_id
  Given a subscription for run "r1" that also receives a "r2" event before an "r1" terminal
  When the live event stream is collected
  Then the body excludes the r2 event and closes on the r1 terminal
