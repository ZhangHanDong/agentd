# Control-Plane Execution Evidence API

- **Date**: 2026-07-10
- **Status**: P271 implementation design
- **Authority basis**: P264 ownership, P265 identities, P268 evidence store, and P270 fencing
- **Scope**: artifact index, audit replay, measured usage, certification references, and fenced worker evidence

## 1. Authority Split

`AgentdControlPlane` owns the execution artifact index, execution audit log,
and measured usage. `OpenFabCertificationAuthority` owns certification
requests, results, signatures, and attestations; agentd may retain immutable
external references to those records. `AgentdWorker` may report evidence but
cannot make itself the durable authority.

P271 exposes four internal Rust ports:

- `ArtifactIndexPort`: publish immutable metadata, exact lookup, and bounded
  run listing.
- `ExecutionAuditPort`: idempotent append and sequence-cursor replay.
- `UsageLedgerPort`: typed usage append, sequence replay, and per-run totals.
- `CertificationReferencePort`: append and list immutable external refs.

These are control-plane APIs, not public HTTP or worker wire schemas.

## 2. Shared Evidence Envelope

Artifacts, audit events, and usage measurements carry the same immutable
execution links:

```text
ExecutionRunId
optional ExecutionTaskId / RuntimeSessionId / RuntimeAttemptId
optional WorkerIncarnationId
execution snapshot authority + kind + id + version + content SHA-256
target repository id + immutable base commit
```

P271 preserves the P268 parent-graph validation. A child link cannot point to
another run, task, session, attempt, worker, snapshot, repository, or base
commit. The API does not infer those links from paths, Matrix ids, runtime
locators, or storage refs.

## 3. Artifact Index

Artifact publication starts only after bytes have a durable opaque
`storage_ref`. P271 records metadata; it does not upload, read, scan, encrypt,
or delete bytes.

`ExecutionArtifactId` is the publication retry key. An exact retry returns the
original immutable row. Reusing the id with a changed envelope returns
conflict. Run listing orders by `(created_at, id)` and uses that tuple as its
cursor because P268 artifact rows have no database sequence.

Pages accept `1..=200` rows. A cursor belongs to the selected run and pages
advance strictly after it.

## 4. Audit and Usage

Audit append keeps P268 semantics: one canonical `AuditEventId`, one
idempotency scope/key, database-assigned sequence, immutable payload hash/JSON,
and exact parent links. Replay orders only by sequence. Caller `occurred_at`
never controls replay.

Measured usage is not a new identity or table. It is an audit event with:

```text
event_type = "usage.measured"
payload = {
  "metric": <closed UsageMetric>,
  "quantity": <u64>,
  "provider": <optional string>,
  "model": <optional string>
}
```

The closed metrics are `input_tokens`, `cached_input_tokens`, `output_tokens`,
`reasoning_tokens`, `tool_calls`, `runtime_milliseconds`, and
`artifact_bytes`. Typed API construction and typed payload parsing prevent
arbitrary audit JSON from entering usage totals. Exact audit idempotency keeps
retries from double counting.

Usage pages reuse audit sequence. Totals are deterministic unsigned sums by
metric for one run. P271 records facts only; P279 applies quota policy and
budget enforcement.

## 5. Certification References

`CertificationReferencePort` records P268 external refs in database id order.
The closed kinds are `request`, `result`, `signature`, and `attestation`.
Exact retries are idempotent and conflicting reuse fails.

This port is deliberately not the P264 OpenFab network transport. It does not
submit, poll, verify a signature, interpret pass/fail, advance product
workflow, or gate delivery. The default remains `gate=none`.

## 6. Fenced Worker Reports

`SqliteExecutionEvidenceControlPlane<L>` receives an injected
`TaskLeasePort`. Worker artifact and usage entry points require:

1. A syntactically valid exact `TaskLeaseClaim`.
2. Evidence task and worker links equal to the claim.
3. Successful P270 `validate_claim` at the control-plane observation time.

An accepted report then enters the corresponding P271 port. A rejected report
does not create artifact or usage state. Before returning a typed rejection,
the adapter appends `execution.report_rejected` with operation, lease id,
fencing token, and rejection reason to P268 audit storage. If that append
fails, the API fails closed as unavailable rather than claiming an audited
rejection.

Each rejected call is an audit fact and receives a new `AuditEventId`; network
retry collapse belongs to the future P278 worker protocol.

## 7. Failure and Follow-On Work

- Invalid links, hashes, payloads, cursors, or limits fail before mutation.
- Changed artifact/audit/usage/certification retries conflict and retain the
  original rows.
- Audit and usage sequence cursors cannot be negative or time-derived.
- OpenFab and object storage outages do not silently change authority or
  delivery state because P271 does not call those systems.

P272 adds runtime status/capture/shutdown/rebind compatibility. P278 adds
authenticated worker pull and evidence acknowledgement/retry. P279 adds
RBAC/quota enforcement and diagnostics. Object storage and real OpenFab
transport remain explicit future integration work, so P271 alone does not make
agentd a complete agent-chat replacement.
