# Enterprise Scale Operations Runbook

- Scope: AD-E7 code candidate and pre-production acceptance
- Authority: agentd owns execution state; Specify owns project/policy state;
  OpenFab owns certification verdicts
- Safety rule: never repair state by rewinding a cursor, lease epoch, fencing
  token, audit head, certification result, or DR checkpoint

## Preconditions

1. Choose the deployment class explicitly. The checked-in profile is a
   single-replica SQLite reference runtime. A multi-replica target requires an
   external replicated durable-store composition covering queue, lease,
   outbox, audit, runtime, Matrix, certification, and enterprise-scale state.
   Do not mount or copy one SQLite file into multiple control-plane replicas.
2. Pin control-plane and worker images by digest and admit them through the
   `agentd-signed-images` policy.
3. Configure operator bearer auth in hard mode, workload identity, Specify
   HTTPS authority/auth, KMS references, object-store references, Matrix, and
   OpenFab. Secret or key bytes must not be placed in agentd JSON or SQLite.
4. Register `factory-load-model-v1` and the active retention policy. Record
   their content SHA-256 values in the acceptance directory.
5. Confirm a current DR checkpoint and two successful backups exist before a
   rollout, region change, key rotation, or failure-injection drill.
6. Confirm the database reports schema `27`; migrations `0024`-`0027` provide
   the base scale ledger, transition history, transactional leadership fencing,
   and immutable policy/rollout audit history.

## Transport And Identity

The reference pod has two Envoy TLS listeners. Port `9443` is the operator
listener and still requires the agentd bearer token. Port `8443` is the worker
listener, requires a client certificate, replaces XFCC, removes caller-supplied
private identity headers, and permits only worker heartbeat/pull/renew/report
paths. The agentd port `8787` must not be reachable outside the pod.

The enterprise operator router leaves only `/healthz`, `/dashboard`, and
`/dashboard/` publicly reachable as non-mutating shell routes. Every other
surface route is protected at composition time. The dashboard exchanges the
operator bearer once at `POST /api/operator/session` for a browser-session
cookie named `__Host-agentd_operator_read`; the cookie is `Secure`, `HttpOnly`,
`SameSite=Strict`, host-only, path `/`, and has no persistent lifetime. It is
accepted only for `GET` and `HEAD`. Every mutation still requires the bearer
header. Never put the bearer or derived cookie in a URL, browser storage,
screenshots, logs, or acceptance artifacts. The Secure cookie requires access
through the operator TLS listener; direct HTTP to the app port is unsupported.

The server certificate must cover both Kubernetes service DNS names. Worker
leaf certificates must have exactly one URI SAN in this form:

```text
spiffe://factory.example/worker/wi_<26-character-ULID>
```

The workload identity CSI provider writes a bounded per-pod descriptor:

```json
{
  "worker_id": "wk_<26-character-ULID>",
  "worker_incarnation_id": "wi_<26-character-ULID>",
  "client_identity_pem": "/var/run/secrets/agentd/workload/client-identity.pem",
  "server_ca_pem": "/var/run/secrets/agentd/workload/server-ca.pem"
}
```

Enroll a verified certificate through the operator listener before starting
the worker. `certificate_chain_der_base64` is leaf first. The attestation is
operator-controlled and the scheduler requires each heartbeat to match it.

```json
{
  "worker_id": "wk_<26-character-ULID>",
  "worker_incarnation_id": "wi_<26-character-ULID>",
  "daemon_version": "0.1.0",
  "host_name": "worker-cn-east-1a-0",
  "network_zone": "cn-east-1a",
  "labels": {
    "agentd_attestation": {
      "rollout_id": "ir_<26-character-ULID>",
      "image_digest": "sha256:<64-lowercase-hex>",
      "signature_bundle_sha256": "<64-lowercase-hex>",
      "signature_policy_sha256": "<64-lowercase-hex>",
      "region": "cn-east-1",
      "zone": "cn-east-1a",
      "resource_class": "standard"
    }
  },
  "capabilities": {"runtime": ["codex"]},
  "certificate_chain_der_base64": ["<leaf>", "<intermediate>"]
}
```

```bash
agentctl enterprise worker-enroll --file worker-enrollment.json \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --server-ca-pem "$AGENTD_OPERATOR_CA_PEM" \
  --api-token "$AGENTD_API_TOKEN"
```

Revocation uses server time and preserves the binding history:

```json
{"certificate_sha256":"<64-lowercase-hex>","reason":"rotation"}
```

```bash
agentctl enterprise worker-identity-revoke --file worker-revocation.json \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --server-ca-pem "$AGENTD_OPERATOR_CA_PEM" \
  --api-token "$AGENTD_API_TOKEN"
```

Every replacement pod receives a new incarnation ID and certificate. A new
incarnation may atomically replace the stable worker's image attestation; old
certificates remain historically resolvable but fail current-incarnation
authorization. A container restart inside the same pod keeps the incarnation
but seeds its heartbeat sequence from Unix nanoseconds so the durable sequence
continues forward. A clock rollback fails closed; operators must replace the
pod and issue a new incarnation rather than editing availability state.

Enrollment requires the referenced rollout and an enabled matching zone policy
to exist first. Use `agentctl enterprise rollout-rollback --file ...` with a
digest-only reason to stop a rollout; the same fenced transaction records the
rollback and offlines every availability row bound to it.

```json
{
  "rollout_id": "ir_<26-character-ULID>",
  "reason_sha256": "<64-lowercase-hex>",
  "rolled_back_at": 1780000000
}
```

## Worker Executor Contract

The worker is outbound-only and has one active slot. It writes one bounded
`agentd-codex-fleet-executor-v1` request to the configured executor's stdin with
a cleared environment and no raw prompt. The request contains the immutable
`FleetAssignment`, protocol version, exact artifact-acknowledgement and
side-effect-admission URLs, transport mode, one lease-specific
`executor_work_dir`, and paths to the mounted client identity/server CA files.
It never embeds certificate, key, credential, transcript, or object bytes.
`--executor-work-root` must be a dedicated writable volume: the worker removes
stale entries before each assignment, creates one `0700` directory keyed by
task/lease/fence, and removes it on completion, error, or cancellation. Do not
place persistent caches or operator files under that root. The external
provider resolves immutable task, snapshot, policy, and artifact references
and must obtain a fresh fenced admission before each protected artifact or
external side effect. The checked-in Kubernetes profile requires the
Codex-only provider and must not install a Claude executor. Model traffic must
traverse the labelled worker egress gateway; direct arbitrary worker internet
egress remains denied. Provider credentials are checked out through the
external secret broker and are not inherited from the worker environment.

The executor emits exactly one JSON result on stdout and no more than 2 MiB:

```json
{"status":"completed","outcome_sha256":"<64-lowercase-hex>"}
```

or:

```json
{"status":"failed","failure_code":"provider_unavailable","retryable":true}
```

Non-zero exit, malformed/oversized output, lost renewal, or shutdown terminates
the executor's isolated process group, including descendants, and reports a
retryable fenced failure when the lease remains valid.

## Normal Inspection

```bash
agentctl enterprise status \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --server-ca-pem "$AGENTD_OPERATOR_CA_PEM" \
  --api-token "$AGENTD_API_TOKEN"

agentctl enterprise explain tr_REPLACE \
  --daemon-url "$AGENTD_ENTERPRISE_URL" \
  --server-ca-pem "$AGENTD_OPERATOR_CA_PEM" \
  --api-token "$AGENTD_API_TOKEN"

agentctl cutover doctor --db-path "$AGENTD_DB_PATH"
```

The SQLite reference profile requires one ready member and one unexpired
leader, but is not HA evidence. An HA target requires at least two ready
members backed by the approved replicated store. Both require every enabled
zone represented, no degraded rollout, replicas within policy, no SLO breach,
and exact explain data for every task.
Queue growth is actionable only when correlated with available slots, quota,
placement policy, and the current Specify epoch.

A restarted control-plane process reuses its configured stable instance id and
resumes the durable heartbeat sequence. That identity cannot change region,
zone, or endpoint; moving an instance requires a new id. Leader-only mutations
also require the member to remain `ready` and the request time to fall between
the current lease renewal and expiry.

The checked-in worker profile declares six independent pools across
`cn-east-1a/b/c` and `cn-north-1a/b/c`. Each pool starts with three replicas,
pins its zone, and uses a hard hostname topology-spread constraint. A zone
without three eligible hosts therefore remains visibly under-capacity instead
of concentrating all replicas on one node.

## Kubernetes Deployment Preflight

The checked-in manifests are a reference profile and intentionally contain
non-production image, identity, rollout, policy, and endpoint placeholders.
Render that profile for local inspection only with the explicit exception:

```bash
bash scripts/agentd_enterprise_deploy_preflight.sh \
  --allow-reference-placeholders \
  --output .agentd/enterprise-preflight/reference/rendered.yaml
```

Acceptance must point `--kustomization` at a production overlay that replaces
every placeholder. It requires API-server admission and writes a new read-only
render plus SHA-256 sidecar; never reuse or overwrite an earlier evidence path.

```bash
bash scripts/agentd_enterprise_deploy_preflight.sh \
  --kustomization "$AGENTD_ENTERPRISE_KUSTOMIZATION" \
  --server-dry-run \
  --output "$AGENTD_ACCEPTANCE_DIR/ad-e7-kubernetes/rendered.yaml"
```

The gate structurally requires exactly one SQLite reference control-plane
replica with one RWO PVC, fixed non-root pod identities, end-to-end TLS health
probes, exactly six region/zone-pinned worker Deployments, three initial
replicas and one hard hostname spread per pool, and one same-named HPA per
Deployment. This validates only the honest reference topology. A production
multi-replica control plane additionally requires the approved replicated
durable-store adapter and its own deployment overlay.

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

Create a replication plan only after its immutable execution-artifact record
exists with the same content digest, then acknowledge replicas. An
acknowledgement must match artifact digest, region, opaque object-ref hash,
tenant key id, and status. Deletion is ineligible until every required region
is available.

Key rotation is forward-only:

1. Transition the active predecessor to `retiring`.
2. Register the successor opaque KMS key/version references as the sole new
   `active` key. Registration cannot create a pre-retired record.
3. Make new writes use the successor through the external KMS policy.
4. Re-encrypt/replicate object bytes outside agentd and submit digest-only
   acknowledgements.
5. Retire the predecessor only after policy replicas and rollback requirements
   are satisfied. Never store key bytes in a mutation file.

## Retention And Legal Hold

Set retention using a content-addressed policy. Place a legal hold before any
subject disposition review. An active hold always wins over elapsed retention
and completed replication. Release requires a distinct operator action and an
immutable release timestamp. Deletion remains an external side effect and must
carry the exact retention decision and current fencing evidence.

## Load Evidence

The guarded load harness runs its driver with a cleared environment and a
2 MiB limit per stdout/stderr stream. Metrics must use
`agentd.enterprise-load-metrics/v1`, repeat the exact load-model SHA-256, and
provide object results for all eleven factory dimensions. Recursive fields
that can carry prompts, transcripts, credentials, keys, certificates, raw
content, artifact bytes, stdout, or stderr are rejected. The immutable evidence
directory retains validated metrics, the pinned model, a manifest, and only the
stderr SHA-256; raw stderr and driver scratch files are deleted.

## Evidence

For every operation retain command name, redacted input SHA-256, response
SHA-256, binary/source revision, image digest, leadership term/fence, observed
time, and linked audit/outbox ids. Do not retain bearer tokens, key bytes,
derived browser cookies, prompts, transcripts, or object contents in the
acceptance directory.
