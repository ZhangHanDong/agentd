spec: task
name: "daemon startup guard — clear already-running detection at bind (P1)"
tags: [daemon, startup, e2e, p1, hardening]
---

## Intent

When a daemon is already running on the configured port, a second `serve` today
fails late with a raw OS error (`Address already in use (os error 48)`) AFTER it
has opened the SQLite store and done startup work. Detect the already-running
case clearly and EARLY, with an actionable message, and before touching shared
state.

This is the herdr "live-vs-stale" borrow — but adapted honestly to TCP. herdr's
try-connect probe exists to solve a UNIX-SOCKET problem: a crashed daemon leaves
a socket FILE that blocks `bind` even though nothing is listening, so it must
probe to tell a stale file from a live listener before binding. TCP has no such
file: `bind` ITSELF is the race-free live-vs-free detector — `AddrInUse` means a
live listener already owns the port, authoritatively and with no TOCTOU window.
So the mechanism here is `bind`, not a probe; the work is (a) mapping its error
to a clear message and (b) binding FIRST so the failure precedes the store open.
The "stale cleanup" half of the borrow is N/A — TCP leaves no artifact to clean.

## Decisions

- Add `bind_listener(addr) -> Result<TcpListener, String>`: bind the TCP
  listener, mapping `io::ErrorKind::AddrInUse` to a clear already-running message
  (naming the address) and passing any other bind error through as its string.
  A seam: tests drive it with real ephemeral ports, no running daemon needed.
- `serve` binds via `bind_listener` as its FIRST step — before
  `build_production_host` (which opens the SQLite store). A second instance thus
  fails fast on the friendly error without opening the DB or doing startup work.
- The friendly message comes from one `already_running_msg(addr)` helper so the
  detection path can't drift from any other reference to it.
- `bind` is the authority: detection is race-free (no probe, no TOCTOU). Port 0
  (`AddrInUse` can't occur) binds an OS-assigned free port — the success path.

## Boundaries

### Allowed Changes

- crates/agentd-bin/src/**
- crates/agentd-bin/tests/**
- specs/e2e/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not add a try-connect probe / read timeout — `bind` is the detector; a
  probe would add a redundant, non-authoritative code path (see Intent).

## Out of Scope

- Stale-artifact cleanup — TCP leaves none (unlike a unix socket file).
- Taking over / killing an existing daemon, or port auto-increment fallback: the
  guard reports and exits; it does not reassign the port.

## Completion Criteria

Scenario: binding a free port succeeds
  Test: bind_listener_succeeds_on_free_port
  Given the loopback address with port 0 (an OS-assigned free port)
  When bind_listener is called
  Then it returns a bound listener

Scenario: a port already owned by a live listener is reported as already-running
  Test: bind_listener_reports_already_running
  Given a TCP listener already bound to a loopback address
  When bind_listener is called for that same address
  Then it returns an error whose message reports the daemon is already running and names the address
