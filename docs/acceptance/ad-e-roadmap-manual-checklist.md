# AD-E Roadmap Final Manual Checklist

- Status: deferred; no item is accepted yet
- Execution policy: run once after AD-E1 through AD-E7 candidate code is present
- Agent runtime for real smoke: Codex only; unset `ANTHROPIC_API_KEY` and `CLAUDE_API_KEY`
- Evidence root: set `AGENTD_ACCEPTANCE_DIR` to a new immutable run directory
- Promotion rule: an AD-E/FSF gate remains open until every required item has evidence, rollback notes, and operator sign-off

## AD-E1

- [ ] OIDC: authenticate a signed token from the configured HTTPS issuer; capture issuer, audience, `azp`, kid, principal id, trusted time, repository-response binding, expiry, and redacted denial evidence for wrong issuer/audience/authorized-party/kid/algorithm/expiry.
- [ ] Matrix: resolve a current human device and a trusted appservice service principal from authenticated Matrix transport metadata; capture trusted event time and denials for forged sender/device/appservice metadata, foreign homeserver, disabled user, missing/revoked device, and foreign appservice namespace.
- [ ] Workload mTLS: rotate a worker certificate and prove expired, revoked, foreign-CA, and superseded-incarnation identities cannot dispatch or renew.
- [ ] Remote secret broker: check out repository/model/object-store credentials through the production transport, prove checkout nonce, RBAC version, revocation epoch, secret version, local timeout, scope/expiry caps, credential rotation, and absence of secret bytes in storage/log/audit.
- [ ] OCI sandbox: run `AGENTD_REAL_SECURITY_SANDBOX_SMOKE=1 bash scripts/agentd_real_security_sandbox_smoke.sh --execute`; prove host credentials, another tenant worktree/cache, privilege escalation, and undeclared egress are denied and cleanup is deterministic.
- [ ] Cross-tenant: attempt API, queue, audit, artifact, cache, model-cache, secret, and worker-reuse access using mismatched organization/project/snapshot references; retain stable denial codes.
- [ ] Redaction: inject exact, overlapping, regex-shaped, UTF-8, and binary secrets into stdout/stderr/transcript/audit paths; prove persisted and delivered content contains deterministic replacement only.
- [ ] Revocation: advance Specify epoch during dispatch, lease renewal, artifact acceptance, delivery, and release; prove every stale operation stops before its external side effect.
- [ ] Placement: exercise classification, region, trust domain, canonical signed image digest, dedicated pool, egress profile, and tenant cache namespace denials against real worker inventory.
- [ ] AD-E1 rollback: restore previous enterprise security configuration without enabling standalone/open-auth fallback on an enterprise listener.
- [ ] FSF-2 operator sign-off recorded.

## AD-E2

- [ ] Restart the control plane during queued, acquired, renewed, releasing, retrying, and artifact-upload states; prove no accepted task or lease is lost.
- [ ] Kill and replace workers across incarnation, heartbeat, drain, offline, capacity, quota, retry, dead-letter, and reaper paths.
- [ ] Prove stale fencing tokens cannot publish outcome, artifact, usage, forge, delivery, release, secret, or high-risk tool side effects.
- [ ] Run duplicate acquisition/release/upload and network-partition failure injection; retain idempotency and operator explain evidence.
- [ ] FSF-3 operator sign-off recorded.

## AD-E3

- [ ] Bind a Robrix project through Specify and dispatch through the native agentd Matrix gateway with no agent-chat process.
- [ ] Replay Matrix sync and command inbox across restart; prove one canonical command id creates at most one run id.
- [ ] Verify inviter/sender/appservice ACL, attachment content addressing, bounded summaries, raw-transcript exclusion, and actionable failure links.
- [ ] Execute observe, shadow-read-only, canary, per-project cursor/authority cutover, drain, rollback, and retire procedures.
- [ ] FSF-4 operator sign-off recorded.

## AD-E4

- [ ] Export a signed evidence envelope and independently verify immutable project/source/spec/artifact/policy/skill digests in OpenFab.
- [ ] Rotate/revoke builder and worker keys; verify historical evidence remains resolvable while new revoked evidence is rejected.
- [ ] Exercise `gate=none`, required machine verification, human/N-of-M certification, delivery, release, and revocation mappings.
- [ ] Install approved Skill Hub packages and deny yanked/revoked/unapproved package versions without rewriting historical evidence.
- [ ] FSF-5 operator sign-off recorded.

## AD-E5

- [ ] Run `AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute` with Codex and no Claude environment variables.
- [ ] Exercise native PTY spawn, text/keys, resize, interrupt, bounded capture, shutdown, idle reap, and explicit outcome submission.
- [ ] Restart the daemon with live/resumable/gone sessions; prove stable session ids, new attempt ids, provider-native resume refs, and explicit `runtime_gone` termination.
- [ ] Compare SSE events, snapshot, wait API, dashboard, Robrix, Matrix summary, and agentctl views for the same runtime.
- [ ] Prove production runtime control has no tmux dependency.
- [ ] FSF-6 native-runtime sign-off recorded.

## AD-E6

- [ ] Complete shadow decision comparison and supported-state import with stable id mappings.
- [ ] Drain in-flight runs, hand off cursors, install local/team/fleet services, and execute doctor/backup/restore/rollback.
- [ ] Confirm parity audit has no required missing/partial row except an explicit approved product-scope decision.
- [ ] Remove agent-chat/tmux production config, startup entrypoints, runtime dependencies, docs, and operator procedures; retain only approved offline import compatibility.
- [ ] Human legacy-removal authorization and FSF-6 final-cutover sign-off recorded.

## AD-E7

- [ ] Run the pinned factory load model across tenants, projects, rooms, Matrix events, queues, artifacts/logs, certification throughput, failures, and noisy neighbors.
- [ ] Lose a worker, control-plane instance, and zone independently; prove no accepted state is lost and lease fencing remains valid.
- [ ] Verify signed Kubernetes image rollout, per-zone pull workers, autoscaling, multi-region artifact replication, and tenant encryption keys.
- [ ] Execute retention, legal hold, disaster recovery, RPO/RTO, capacity, backlog, budget, failure, and SLO operator drills.
- [ ] FSF-7 operator sign-off recorded.

## Final Decision

- [ ] Every evidence path is immutable and linked from this checklist.
- [ ] All rollback procedures were executed, not only reviewed.
- [ ] Required security, product, operations, and migration owners signed the final record.
- [ ] Only after the prior items pass, update roadmap/parity gates from candidate to accepted.
