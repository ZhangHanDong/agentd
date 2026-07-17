# Enterprise Scale Operations Runbook

- Scope: AD-E7 code candidate and pre-production acceptance
- Authority: agentd owns execution state; Specify owns project/policy state;
  OpenFab owns certification verdicts
- Safety rule: never repair state by rewinding a cursor, lease epoch, fencing
  token, audit head, certification result, or DR checkpoint

## Preconditions

1. Deploy an external replicated durable-store adapter implementing the
   agentd store/enterprise ports. Do not mount one SQLite file into multiple
   control-plane replicas.
2. Pin control-plane and worker images by digest and admit them through the
   `agentd-signed-images` policy.
3. Configure operator bearer auth in hard mode, workload identity, Specify
   HTTPS authority/auth, KMS references, object-store references, Matrix, and
   OpenFab. Secret or key bytes must not be placed in agentd JSON or SQLite.
4. Register `factory-load-model-v1` and the active retention policy. Record
   their content SHA-256 values in the acceptance directory.
5. Confirm a current DR checkpoint and two successful backups exist before a
   rollout, region change, key rotation, or failure-injection drill.

## Normal Inspection

```bash
agentctl enterprise status \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --api-token "$AGENTD_API_TOKEN"

agentctl enterprise explain tr_REPLACE \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --api-token "$AGENTD_API_TOKEN"

agentctl cutover doctor --db-path "$AGENTD_DB_PATH"
```

Healthy operation requires one unexpired leader, at least two ready
control-plane members, every enabled zone represented, no degraded rollout,
replicas within policy, no SLO breach, and exact explain data for every task.
Queue growth is actionable only when correlated with available slots, quota,
placement policy, and the current Specify epoch.

## Signed Rollout

1. Verify the registry reference contains `@sha256:` and the Sigstore bundle
   and policy hashes are available.
2. Submit the rollout JSON with `agentctl enterprise rollout --file ...`.
3. Update one canary zone. Submit each zone observation with
   `rollout-observe`; do not mark signature verification true based only on a
   successful pull.
4. Continue zone by zone only while dead-letter, denial, latency, and budget
   measurements remain within objectives.
5. Stop on digest mismatch, signature failure, stale lease acceptance,
   unexplained task, replica regression, or SLO breach. Roll back by declaring
   the prior signed digest as a new forward rollout; never rewrite history.

## Autoscaling And Zone Pools

Zone policy is changed through `zone-policy`; observations are submitted
through `capacity`. The returned desired count is an audited recommendation.
The HPA/external controller may apply it only within the policy minimum,
maximum, cooldown, trust domain, resource class, rollout, region, and zone.

For backlog incidents:

1. Inspect queue age, queue depth, running tasks, ready workers, available
   slots, quota denials, placement denials, and Specify availability.
2. Scale only a pool whose signed digest and trust domain match the pending
   tasks.
3. Do not bypass quota, placement, snapshot expiry, or revocation to clear a
   queue.

## Failure Drills

### Worker Loss

Delete one worker pod while it owns a lease. Verify the incarnation becomes
unavailable, the lease expires/reaps, retry receives a higher fencing token,
and the old worker cannot submit outcome, artifact, usage, delivery, or side
effects. Accepted task count and outbox history must not decrease.

### Control-Plane Instance Loss

Terminate the current leader. Verify a surviving member acquires a higher term
and fencing token, API reads remain available, stale leader mutations return a
conflict, and accepted queue/lease/outbox/audit state is unchanged. Stop if the
replicated store cannot prove quorum durability.

### Zone Loss

Cordon and remove all workers and one control-plane member in a zone. Verify
other zones respect placement and data-residency policy, no task is silently
moved to an ineligible zone, and recovery meets the pinned load model RPO/RTO.
After recovery, workers must use new incarnations and current signed images.

### Specify Outage

Block Specify transport. New project admission, policy refresh, and stale epoch
operations must fail closed. Existing acknowledged state remains readable;
agentd must not switch to local authority. Restore transport, validate the
authority key and epoch, then resume admission without replaying accepted
commands.

### KMS Or Object-Store Outage

KMS failure blocks new key-dependent artifact publication. Object-store failure
blocks replica acknowledgement. Neither permits deletion or marks a replica
available. Recover the dependency, retry with the same immutable digest and
idempotency identity, and confirm no key material or object bytes entered the
agentd ledger.

## Replication And Tenant Keys

Create a replication plan before acknowledging replicas. An acknowledgement
must match artifact digest, region, opaque object-ref hash, tenant key id, and
status. Deletion is ineligible until every required region is available.

Key rotation is forward-only:

1. Register the successor opaque KMS key/version references.
2. Make new writes use the successor through the external KMS policy.
3. Re-encrypt/replicate object bytes outside agentd and submit digest-only
   acknowledgements.
4. Retire the predecessor only after policy replicas and rollback requirements
   are satisfied. Never store key bytes in a mutation file.

## Retention And Legal Hold

Set retention using a content-addressed policy. Place a legal hold before any
subject disposition review. An active hold always wins over elapsed retention
and completed replication. Release requires a distinct operator action and an
immutable release timestamp. Deletion remains an external side effect and must
carry the exact retention decision and current fencing evidence.

## Evidence

For every operation retain command name, redacted input SHA-256, response
SHA-256, binary/source revision, image digest, leadership term/fence, observed
time, and linked audit/outbox ids. Do not retain bearer tokens, key bytes,
prompts, transcripts, or object contents in the acceptance directory.

