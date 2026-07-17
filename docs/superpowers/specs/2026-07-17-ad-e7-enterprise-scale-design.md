# AD-E7 Enterprise Scale Design

- Status: approved implementation design; acceptance deferred
- Date: 2026-07-17
- Canonical source: `docs/plans/2026-07-09-agentd-native-runtime-roadmap.md`

## Goal

Scale the durable AD-E2/AD-E4/AD-E6 contracts to a multi-instance,
multi-zone, multi-region enterprise deployment without weakening workload
identity, project authority, lease fencing, evidence, or native runtime
ownership.

## Boundaries

Agentd owns execution admission and durable operational state. Specify remains
the project authority and is reached through an authenticated HTTPS adapter.
OpenFab remains the certification authority. Object stores and key-management
systems retain bytes and key material; agentd stores immutable object digests,
replica acknowledgements, and opaque tenant key references only.

Workers pull assignments over authenticated outbound connections. No worker
requires an inbound listener. Kubernetes is a deployment profile, not a new
scheduler or identity model.

## Architecture

### Enterprise Scale Port

`EnterpriseScalePort` is the logical control-plane boundary for:

- control-plane membership and a fenced, expiring leadership lease;
- signed worker-image rollout declarations and per-zone observations;
- per-zone pool policy and deterministic autoscaling recommendations;
- artifact replica acknowledgement and tenant encryption-key references;
- retention policies, legal holds, and disaster-recovery checkpoints/drills;
- pinned factory load-model registration and bounded operational snapshots.

Every mutation is idempotent by immutable identity or explicit idempotency key.
Conflicting replay fails closed. Leadership terms and fencing tokens increase
monotonically, and expired leaders cannot renew or mutate leader-only state.

### Storage

Migration `0024` adds normalized tables for each enterprise resource. Migration
`0025` preserves immutable tenant-key and replica transition history, `0026`
records every leader-only mutation fence in the same write transaction as its
state change, and `0027` preserves rollout observation/rollback, zone-policy,
and retention-policy history while prohibiting destructive ledger mutation.
The SQLite implementation is the standalone/reference adapter and
exercises exact transaction semantics. The port is storage-neutral so an HA
deployment can provide a replicated adapter without changing API or worker
contracts. The checked-in Kubernetes reference deliberately runs one replica
with one RWO volume; it must not be scaled until the full durable-store
composition is replaced.

### Specify Transport

`HttpSpecifyAuthorityTransport` uses HTTPS, bounded timeouts, an injected
workload-identity authorization header, exact JSON response limits, and the
existing `SpecifyProjectAuthority` envelope validation. It never falls back to
local authority. Transport failures remain typed unavailable errors.

### Fleet And Rollout

AD-E2 remains the only lease/fencing scheduler. AD-E7 zone policies select
minimum/maximum capacity, target queue-per-slot, cooldown, signed image digest,
trust domain, and region. Autoscaling emits recommendations and audit receipts;
Kubernetes HPA or an external controller applies them. A rollout is healthy only
when every required zone reports the declared digest and verified signature.

### Replication, Keys, And Compliance

Artifact replication tracks content digest, source region, required regions,
opaque object refs, and acknowledgements. Tenant key records contain only KMS
key/version references and rotation state. Retention computes disposition from
the pinned policy; an active legal hold always prevents deletion. DR checkpoints
pin database, artifact-index, audit, Matrix cursor, and certification digests,
plus declared RPO/RTO. Drill records compare measured recovery without rewriting
the checkpoint.

### Operations

HTTP and `agentctl enterprise` expose a redacted operational snapshot and exact
task explain data. The dashboard adds a compact enterprise view for leadership,
zones, backlog, replication, budget, failures, and SLO status. No endpoint emits
credentials, prompt text, transcript bytes, or tenant key material.

In enterprise composition, only health and the dashboard HTML shell are public.
The operator bearer authorizes all surface methods; a bearer-authenticated
session endpoint can issue a host-only Secure/HttpOnly/SameSite=Strict cookie
for browser `GET`/`HEAD` requests. Dashboard code never stores the bearer or
places it in a URL, and the cookie cannot authorize a mutation. Fleet routes
remain outside this operator gate and retain their exact mTLS identity contract.

Checked-in Kubernetes assets include one honest SQLite reference control plane,
separate operator TLS and worker mTLS listeners, zone-scoped outbound pull
workers, deny-by-default network policy, HPA, external CSI identity and
Codex-executor contracts, per-lease disposable work directories, digest-only
images, and Sigstore policy-controller intent. The six checked-in zone pools
spread their replicas across hostnames. Three-replica control-plane deployment
remains an external replicated-store integration and acceptance task.

## Failure Semantics

- Leadership acquisition uses one durable transaction; stale term/fence renewals
  fail with conflict.
- A zone or instance loss changes availability and scaling evidence but cannot
  alter accepted task/lease state.
- Replica acknowledgement requires exact artifact digest and region; deletion is
  denied until policy replicas are complete and no legal hold is active.
- Specify, KMS, object store, Matrix, and OpenFab outages fail closed at their
  corresponding admission boundary while preserving queued/acknowledged state.
- DR restore is operator initiated and forward-only; audit and fencing epochs are
  never rewound.

## Acceptance

Implementation is a candidate until the final unified pass executes unit,
integration, load, failure-injection, Kubernetes policy, browser, and Codex-only
runtime checks. The factory load model must pin its version and cover tenant,
project, room, Matrix event, queue, artifact/log, certification throughput,
failure injection, test window, and noisy-neighbor dimensions.
