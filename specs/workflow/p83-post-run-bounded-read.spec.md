spec: task
name: "agentctl post_run — bounded daemon-response read (P1 hardening)"
tags: [cli, agentctl, p1, hardening, path-b]
---

## Intent

Harden `agentctl run start`'s live path (`post_run`) so a hostile or buggy
daemon cannot OOM the client. Today `post_run` reads the daemon's HTTP response
with `read_to_string`, which buffers an UNBOUNDED number of bytes — a peer that
streams forever exhausts the client's memory. Replace it with a bounded read
that caps the buffered response and parses the status line + body from the
capped bytes. The daemon's real reply is a tiny JSON (`{run_id, status}`), so a
small cap loses nothing on the happy path.

This is the herdr borrow ("never let a peer's unbounded output OOM you") applied
at agentctl's one outbound socket read.

## Decisions

- Extract a `read_response(stream: impl Read) -> Result<(u16, String), String>`
  seam from `post_run`: it performs the bounded read and parses `(status_code,
  body)`. `post_run` keeps owning connect + write, then delegates the read to
  this seam — so the read/parse logic is unit-testable over an in-memory reader
  (a `Cursor`) WITHOUT opening a socket (seam + fake, the project's TDD model).
- Bound the read with `Read::take(MAX_RESPONSE_BYTES + 1)` into a `Vec`, then if
  the buffered length `> MAX_RESPONSE_BYTES` return an error
  (`"daemon response exceeds {MAX_RESPONSE_BYTES} bytes"`). `MAX_RESPONSE_BYTES =
  64 * 1024` — orders of magnitude above the real reply, small enough to bound
  memory.
- On overflow the read ERRORS rather than truncating-and-parsing: parsing a
  truncated body risks accepting half a JSON object as a complete response.
  Losing the status code on a 64-KiB anomaly is the acceptable trade.
- Status-line + body parsing is unchanged (first line's 2nd token → `u16`; body
  is everything after the first `\r\n\r\n`); a response with no parseable status
  line is a `"malformed daemon response"` error, as before.

## Boundaries

### Allowed Changes

- crates/agentctl/src/**
- crates/agentctl/tests/**
- specs/workflow/**

### Forbidden

- Do not modify any file under crates/agentd-core/** (D1).
- Do not change the daemon (surface/bin) response shape — this is a client-read
  hardening only.

## Out of Scope

- A read TIMEOUT / liveness bound. The cap bounds MEMORY, not time: a daemon
  that sends bytes slowly or never closes the connection still blocks the client
  (the pre-existing socket has no read deadline). Closing that hostile-daemon
  threat is deliberately left for a later transport pass — the bound here is
  half the fix (the OOM half), by design.
- Streaming/chunked-transfer decoding — the daemon replies `Connection: close`
  with a small body; the read is read-to-cap-or-EOF.

## Completion Criteria

Scenario: a well-formed small response parses to its status and body
  Test: read_response_parses_status_and_body
  Given an in-memory reader holding a valid `HTTP/1.1 201 Created` response with a small JSON body
  When read_response reads it
  Then it returns the status code 201 and the JSON body

Scenario: an over-cap response is rejected by the memory bound
  Test: read_response_rejects_oversized
  Given an in-memory reader holding a valid status line followed by a body larger than MAX_RESPONSE_BYTES
  When read_response reads it
  Then it returns an error whose message reports the response exceeds the cap (the bound fired, not the parser)

Scenario: a response with no parseable status line is a malformed error
  Test: read_response_malformed_status_is_error
  Given an in-memory reader holding bytes with no HTTP status line
  When read_response reads it
  Then it returns a malformed-response error
