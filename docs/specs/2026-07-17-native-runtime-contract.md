# Agentd Native Runtime Contract

Status: AD-E5 code-complete candidate; not accepted.

## Ownership

- `RuntimeSessionId` is the stable logical identity. It survives process loss
  and provider-native resume.
- `RuntimeAttemptId` identifies one worker-bound process. Every resumed process
  receives a new attempt id.
- Provider-native session references are recovery material, never agentd
  identity and never a substitute for session/attempt fencing.
- `RuntimeBackend` owns only live PTY/process handles. `RuntimeLedgerPort` owns
  durable state, event order, recovery history, and transcript metadata.
- Production native composition uses `portable-pty`, SQLite, the AD-E1
  interactive OCI sandbox, content redaction, Specify epoch checks, and a
  content-addressed transcript directory. It does not use tmux.

## Security

- Every launch and mutation is checked against trusted time, exact execution
  snapshot, task lease, current worker incarnation, sandbox id/profile digest,
  sandbox expiry, capability scope, and current Specify revocation epoch.
- PTY output is redacted before entering event payloads, snapshot tails, or
  transcript bytes.
- Text/key input bytes are written to the live PTY only. SQLite stores the
  idempotency key, SHA-256, byte count, event id, and acceptance time; it does
  not store input text.
- Environment values participate in the command digest but are not stored in
  runtime registration, events, or debug output.
- Transcript objects are immutable `sha256:<digest>` blobs. SQLite stores their
  exact digest, size, truncation flag, archive time, session, and attempt.

## Lifecycle

1. Register one immutable logical session.
2. Begin one current attempt on a current worker incarnation.
3. Construct the provider command, then wrap it in the admitted OCI profile.
4. Spawn the PTY process and atomically mark the attempt/session running.
5. Append redacted semantic events with a session-global monotonic cursor.
6. Capture a bounded provider-native reference when emitted by the CLI.
7. On normal/controlled exit, archive the redacted transcript and terminate the
   durable session with an exact reason.
8. On host loss, recover as `live`, `resumable`, or `runtime_gone`. Resumable
   sessions become `resume_pending`; gone sessions become `lost` with reason
   `runtime_gone`.

No recoverable state may remain implicit after daemon startup reconciliation.

## Surfaces

- Snapshot: `GET /api/runtime/sessions/:id`
- Semantic SSE: `GET /api/runtime/sessions/:id/events`
- Bounded wait: `GET /api/runtime/sessions/:id/wait`
- Controls: text, key, resize, interrupt, and shutdown under the same session
  resource.
- `agentctl runtime` uses those HTTP APIs.
- Robrix/Matrix task projections expose session/attempt/provider/status/cursor,
  resumability, and transcript reference only. They exclude prompts, raw PTY
  output, capabilities, and transcript contents.

## Deferred Acceptance

The final checklist must execute fake lifecycle/store tests, boundary-redaction
tests, the Codex-only `scripts/agentd_native_runtime_smoke.sh`, real OCI plus
Codex/MCP smoke, daemon restart/recovery drills, cross-
surface comparison, and tmux dependency inspection. Only signed evidence from
that pass can close AD-E5/FSF-6.
