# Disaster Recovery Runbook

- Recovery model: operator initiated, forward-only
- Required invariant: accepted state, audit history, cursors, lease epochs, and
  fencing tokens are never rewound
- Default objectives: use the values pinned in the active factory load model
- Store boundary: the checked-in SQLite profile supports single-authority
  backup/restore drills only; multi-member or multi-region acceptance requires
  the external replicated durable-store composition

## Checkpoint Contract

A checkpoint records immutable SHA-256 values for the durable database,
artifact index, audit head, Matrix cursor, and certification head, plus region,
RPO, RTO, and creation time. Object bytes and secrets remain in their owning
systems.

Create the checkpoint JSON only after the database snapshot, object inventory,
audit head, Matrix cursor, and OpenFab head are mutually consistent, then run:

```bash
agentctl enterprise dr-checkpoint --file checkpoint.json \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --api-token "$AGENTD_API_TOKEN"
```

## Disaster Declaration

1. Stop new admission in the affected region without cancelling accepted work.
2. Record incident time, last confirmed checkpoint, current leadership term and
   fence, active leases, queue/outbox heads, Matrix cursor, and artifact replica
   status.
3. Fence affected workers and control-plane instances. Do not reuse their
   incarnation ids or leases.
4. Select a recovery region that satisfies tenant placement, key, object-store,
   Matrix, Specify, and OpenFab policy.

## Restore

1. Restore the durable store from the selected checkpoint into an isolated
   recovery target. Verify bytes before opening it for writes.
2. Reconcile artifact index entries against required-region object refs and
   tenant KMS versions. Missing replicas remain pending.
3. Verify audit, Matrix cursor, and certification heads exactly match the
   checkpoint. Never choose an earlier cursor to make replay easier.
4. Start one recovery control-plane instance with ingress closed. Require
   schema `27`, then run schema,
   integrity, leadership, lease, queue, runtime, Matrix, OpenFab, artifact,
   replication, load-model, and SLO doctor checks.
5. For an approved replicated-store target, start additional members and
   acquire a new higher leadership term/fence. For the SQLite reference target,
   keep exactly one member. Then enable operator reads.
6. Recreate workers with new incarnation ids. Reap old leases before dispatch.
7. Open task admission only after stale workers and old-region writes are
   denied and the current Specify epoch is available.

## Verification

Measure RPO from the latest accepted durable event not present after restore;
the required value is zero unless a separately approved policy says otherwise.
Measure RTO from disaster declaration to restored admission. Verify:

- accepted task, lease, outbox, audit, artifact, Matrix, and certification
  counts and heads;
- old fencing tokens cannot mutate outcome, artifact, delivery, usage, Forge,
  release, or secret state;
- legal holds remain active and retention cannot delete incomplete replicas;
- operators can explain every queued, acquired, retry, dead-letter, completed,
  failed, and cancelled task;
- Palpo/Matrix and Robrix projections resolve to the restored canonical ids.

Record the drill only after the evidence digest is immutable:

```bash
agentctl enterprise dr-drill --file drill-result.json \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --api-token "$AGENTD_API_TOKEN"
```

A passed result requires both accepted-state and lease-fencing verification and
measured RPO/RTO within the checkpoint objectives.

## Failback

Failback is another forward recovery, not a rewind. Create a fresh checkpoint
in the recovery region, replicate it to the repaired primary, repeat all restore
verification, allocate new member/worker identities, and transfer admission.
Keep the incident and both checkpoint/drill records immutable.

## Abort Conditions

Keep admission closed and escalate if any digest mismatches, accepted state is
missing, an old fence can mutate, a legal hold is absent, key/object refs cannot
be resolved, Specify authority changes unexpectedly, or measured RPO/RTO exceeds
the declared objective.
