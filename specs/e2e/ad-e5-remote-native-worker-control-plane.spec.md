---
name: "AD-E5 remote native worker control-plane adapter"
tags: [e5, native-runtime, worker-fleet, artifacts, recovery]
---

# Intent

The remote worker must execute native processes without opening the daemon
SQLite database or reconstructing runtime identity from local files. Runtime
sessions, attempts, leases, evidence, and artifact acknowledgement remain
control-plane-owned.

## Scenarios

### Scenario: remote worker receives a complete execution context

Given a current fenced lease for a native task
And a durable runtime session bound to an immutable project snapshot
When the worker pulls the lease over the authenticated fleet protocol
Then the response contains the runtime session id, execution spec, security scope, and fencing claim
And the scope claim matches the lease id, task id, worker incarnation, and fencing token

### Scenario: remote worker does not use a local shadow database

Given a worker process with no local runtime-session row
When it starts a pulled native task
Then it resolves session/attempt/lease operations through the control-plane adapter
And it does not create a second durable session or attempt locally

### Scenario: artifact upload and acknowledgement remain control-plane owned

Given a native process exits and its output is spooled
When the worker uploads the content-addressed bytes and submits the evidence acknowledgement
Then the control plane validates the lease claim and fencing token
And duplicate upload/ack retry is idempotent
And a rejected acknowledgement cancels or reassigns the lease without publishing stale evidence

### Scenario: daemon outage preserves worker retry state

Given a worker with an active native process
When the control-plane connection is unavailable during heartbeat or acknowledgement
Then the worker retries with bounded backoff and preserves the same attempt and artifact identity
And after reconnect it re-registers the incarnation before resuming control-plane calls
And terminal authentication or fencing errors stop the worker callback

## Boundaries

- The SQLite adapter is used by the daemon control plane only.
- The HTTP adapter is used by remote workers only.
- No remote worker endpoint may accept a worker-local runtime/session identity.
- No Claude process is required by this spec; Codex is the test provider.

## Exit evidence

- fake HTTP control-plane integration test covering session/attempt, lease,
  artifact upload, acknowledgement, outage, reconnect, and fencing rejection;
- daemon SQLite adapter test covering the same port contract;
- Codex-only real smoke evidence using the production object-store selection;
- no production execution path depends on tmux session names or worker-local
  copies of control-plane runtime records.
