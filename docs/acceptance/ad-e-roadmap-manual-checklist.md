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
- [ ] Inspect queue/lease/outbox transaction boundaries during forced process termination; prove each accepted acquisition has exactly one current lease, increasing fencing token, and durable outbox event.
- [ ] Exercise protocol-version mismatch, stale heartbeat sequence/time, placement mismatch, expired snapshot, advanced revocation epoch, quota exhaustion, and zero-capacity pulls; retain stable structured denial/block codes.
- [ ] Interrupt multipart artifact upload before and after acknowledgement; prove publication requires the exact upload id, artifact digest, task, incarnation, lease, fencing token, and artifact-acceptance epoch.
- [ ] FSF-3 operator sign-off recorded.

## AD-E3

- [ ] Bind a Robrix project through Specify and dispatch through the native agentd Matrix gateway with no agent-chat process.
- [ ] Replay Matrix sync and command inbox across restart; prove one canonical command id creates at most one run id.
- [ ] Race command ingress against observe/shadow/canary/active/drain/rollback transitions; prove mode, ACL, snapshot, previous cursor, command, optional run, outbox, and next cursor commit as one transaction.
- [ ] Verify inviter/sender/appservice ACL, attachment content addressing, bounded summaries, raw-transcript exclusion, and actionable failure links.
- [ ] Stop after Matrix send and before delivery acknowledgement, then restart; prove the semantic outbox retries by canonical outbox id and no execution is duplicated.
- [ ] Compare Robrix project/run/task/artifact/approval/evidence projections with the canonical agentd records and prove prompts, answers, findings, logs, and transcripts are absent.
- [ ] Execute observe, shadow-read-only, canary, per-project cursor/authority cutover, drain, rollback, and retire procedures.
- [ ] Resolve every digest-only project/room/principal/task/message/cursor/run mapping in the rollback manifest and verify in-flight ownership returns to the intended control plane.
- [ ] FSF-4 operator sign-off recorded.

## AD-E4

- [ ] Run `cargo test -p agentd-security --test evidence_signing`, `cargo test -p agentd-store --test openfab_certification`, and `cargo test -p agentd-bin --test openfab`; retain complete output and binary/source revisions.
- [ ] Export a signed evidence envelope and independently verify immutable project/source/spec/artifact/policy/skill digests in OpenFab.
- [ ] Capture and compare canonical payload bytes, payload SHA-256, Ed25519 signature, signer DID/key id/role, trusted signing window, snapshot ref, evidence storage ref, OpenFab signed ref, and independently calculated subject/spec/policy/skill digests.
- [ ] Rotate/revoke Builder, Worker, and OpenFab trust keys; verify evidence signed before revocation remains resolvable, signatures at/after revocation are denied, successor keys are used, and no private key enters SQLite/log/audit.
- [ ] Interrupt before/after request persistence, HTTP submission, OpenFab result publication, result polling, and result persistence; prove request/result events replay independently without duplicate certification or mismatched idempotency acceptance.
- [ ] Exercise `gate=none` with unavailable/failing optional certification and prove delivery/release remains non-blocking; then exercise required machine and human/N-of-M gates and prove absent/fail/stale/revoked/wrong-subject/wrong-policy/wrong-snapshot results block Forge merge/release.
- [ ] Walk produced, delivered, machine-attested, human-certified, released, and revoked state mappings; reject stale previous-state and illegal transition replays.
- [ ] Install approved/signed Skill Hub packages pinned by exact archive/manifest/dependency-lock/permissions hashes; deny draft/in-review/yanked/revoked/deprecated, expired, mutable, wrong-version, wrong-hash, and stale trust records.
- [ ] Yank and revoke an installed package, then prove the signed trust observation at install remains historically verifiable while a new install is denied.
- [ ] Verify HTTPS/bearer/timeout/no-redirect/1-MiB response bounds and credential redaction; exercise 401/403/404/409/429/5xx and malformed/mismatched response handling.
- [ ] FSF-5 operator sign-off recorded.

## AD-E5

- [ ] Run `cargo test -p agentd-runtime --test provider_archive`, `cargo test -p agentd-runtime --test native_runtime`, and `cargo test -p agentd-store --test native_runtime_control_plane`; retain complete output and binary/source revisions.
- [ ] Run `AGENTD_REAL_NATIVE_RUNTIME_SMOKE=1 bash scripts/agentd_native_runtime_smoke.sh`; retain Codex PTY output, captured native thread id, terminal status, and content-addressed transcript evidence.
- [ ] Run `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute` with `ANTHROPIC_API_KEY` and `CLAUDE_API_KEY` unset.
- [ ] Exercise native PTY spawn, text/keys, resize, interrupt, bounded capture, shutdown, idle reap, and explicit outcome submission.
- [ ] Inject secrets across PTY read boundaries and prove no unredacted bytes enter semantic events, snapshot tails, Matrix/Robrix summaries, SQLite, or transcript objects.
- [ ] Restart the daemon with live/resumable/gone sessions; prove stable session ids, new attempt ids, provider-native resume refs, and explicit `runtime_gone` termination.
- [ ] Compare SSE events, snapshot, wait API, dashboard, Robrix, Matrix summary, and agentctl views for the same runtime.
- [ ] Replay text/key/resize/interrupt/shutdown idempotency keys before and after daemon restart; prove text is never stored and digest/key conflicts fail closed.
- [ ] Prove production runtime control has no tmux dependency.
- [ ] FSF-6 native-runtime sign-off recorded.

## AD-E6

- Candidate inventory: migrations `0022`/`0023`, `CutoverService`,
  `agentctl cutover`, native daemon composition, `agentd-worktree`, checked-in
  local/team/fleet assets, and `docs/operations/final-cutover-runbook.md`.
- [ ] Complete shadow decision comparison and supported-state import with stable id mappings.
- [ ] Drain in-flight runs, hand off cursors, install local/team/fleet services, and execute doctor/backup/restore/rollback.
- [ ] Confirm the real execute and native runtime smokes invoke Codex only, with Claude credentials unset and no tmux dependency.
- [ ] Confirm parity audit has no required missing/partial row except an explicit approved product-scope decision.
- [ ] Remove agent-chat/tmux production config, startup entrypoints, runtime dependencies, docs, and operator procedures; retain only approved offline import compatibility.
- [ ] Human legacy-removal authorization and FSF-6 final-cutover sign-off recorded.

## AD-E7

- Candidate inventory: migrations `0024`-`0027`, `EnterpriseScalePort`, SQLite
  reference adapter, Specify HTTPS transport, enterprise coordination/profile,
  native fleet HTTP and outbound worker, atomic certificate enrollment,
  `/api/enterprise`, enterprise operator route gate and read-only browser
  session, `agentctl enterprise`, dashboard/doctor, Envoy dual-listener and
  Kubernetes profiles, factory load/retention inputs, runbooks, and guarded load
  harness.
- [ ] Record `git rev-parse HEAD`, `rustc -Vv`, `cargo -V`, Codex version, Kubernetes version, policy-controller version, image digests, provider revisions, and the replicated durable-store adapter/version. Refuse multi-replica acceptance if the target uses one shared SQLite file.
- [ ] Render the checked-in reference profile with `bash scripts/agentd_enterprise_deploy_preflight.sh --allow-reference-placeholders --output "$AGENTD_ACCEPTANCE_DIR/ad-e7-kubernetes/reference.yaml"`; retain its read-only SHA-256 sidecar as non-production topology evidence only.
- [ ] Render the production overlay with `bash scripts/agentd_enterprise_deploy_preflight.sh --kustomization "$AGENTD_ENTERPRISE_KUSTOMIZATION" --server-dry-run --output "$AGENTD_ACCEPTANCE_DIR/ad-e7-kubernetes/rendered.yaml"`; prove strict placeholder rejection, API-server admission, fixed non-root identities, end-to-end TLS health probes, exact six worker/HPA mappings, region/zone node placement, hard hostname spreading, and immutable digest evidence.
- [ ] Confirm the SQLite reference renders exactly one control-plane replica with one RWO PVC. For HA acceptance, replace the complete durable-store composition and prove queue, lease, outbox, audit, runtime, Matrix, certification, and scale state share the replicated authority before increasing replicas.
- [ ] Run `cargo test -p agentd-store --test enterprise_scale`, `cargo test -p agentd-store --test worker_enrollment`, `cargo test -p agentd-store --test enterprise_fleet_scheduler`, `cargo test -p agentd-security --test workload_identity`, `cargo test -p agentd-project-authority --test http_specify_authority`, `cargo test -p agentd-bin --test enterprise_coordination`, `cargo test -p agentd-bin --test enterprise_fleet`, `cargo test -p agentd-bin --test daemon_http`, `cargo test -p agentd-surface --test enterprise --features test-support`, `cargo test -p agentd-surface --test http --features test-support`, and `cargo test -p agentctl --test enterprise_cli`; retain output.
- [ ] Run `kubectl kustomize deploy/enterprise`, policy/schema validation, server-side dry-run, and Sigstore admission against the exact digest-pinned images. Reject placeholder digests, keys, endpoints, KMS refs, or state-adapter configuration.
- [ ] Verify the Envoy configuration validates, the server certificate covers both service DNS names, worker port `8443` requires a trusted client certificate, operator port `9443` preserves bearer auth, app port `8787` is unreachable externally, and only exact worker methods route through the mTLS listener.
- [ ] Through operator TLS, prove `/healthz` and the dashboard shell load without credentials while `/runs`, runtime/SSE, enterprise, Matrix, task, agent, and tool routes return `401` without an operator credential. Prove the login exchange sets only `__Host-agentd_operator_read` with `Secure`, `HttpOnly`, `SameSite=Strict`, path `/`, no `Domain`, and no persistent lifetime; the response must not contain the bearer.
- [ ] Prove the dashboard stores no bearer in URLs, `localStorage`, or `sessionStorage`; same-origin SSE works with the HttpOnly session. Prove the cookie authorizes only `GET`/`HEAD`, every `POST`/`PATCH`/`DELETE` still returns `401` without a bearer, and the fleet mTLS routes remain governed by worker identity rather than the operator cookie.
- [ ] Run `agentctl enterprise status --server-ca-pem ...` against the private operator CA; reject a foreign CA, a hostname/SAN mismatch, redirects, and loopback HTTP combined with a CA file.
- [ ] Send caller-controlled XFCC, `x-agentd-peer-certificate-chain`, and proxy-authorization headers through both listeners. Prove Envoy replaces/removes them and a worker cannot authenticate as another enrolled public certificate chain.
- [ ] Enroll a leaf-first certificate chain with the operator API and prove worker/incarnation/binding/attestation commit atomically. Replay it exactly, reject changed fingerprint reuse, rotate to a new incarnation/image attestation, then revoke the old certificate using server time.
- [ ] Roll back a live rollout through `agentctl enterprise rollout-rollback`; prove the rollback reason/history and leadership fence are immutable, every bound worker is immediately offline, old heartbeat/pull fails, and a new signed rollout plus incarnation is required for recovery.
- [ ] Start the real `enterprise-worker` with the CSI identity descriptor and pinned server CA. Exercise heartbeat, pull, renewal, completion, retryable failure, malformed/oversized executor output, lease loss, SIGTERM, offline heartbeat, and whole executor process-group cleanup using the Codex-only executor provider.
- [ ] Seed stale files and a symlink under the dedicated executor work root, then run two assignments for different tenants. Prove each provider receives only its lease-specific `executor_work_dir`, stale entries are removed without following the symlink, and no work directory survives completion, failure, or cancellation.
- [ ] Prove worker network policy permits only control-plane mTLS, DNS, and the labelled HTTPS egress gateway; prove direct arbitrary internet egress and worker ingress remain denied while the Codex provider can reach its approved model endpoint through the gateway.
- [ ] Inspect executor stdin/stdout, process environment, SQLite, logs, and acceptance artifacts; prove input contains immutable refs, exact admission URLs, transport mode, and TLS file paths but no raw prompt, credentials, certificate/private-key bytes, transcript, or object bytes. Exercise artifact and side-effect calls and prove each is admitted against the current lease and epoch.
- [ ] Register the exact factory model and retention policy through `agentctl enterprise`; retain content SHA-256 values and redacted responses.
- [ ] Run `AGENTD_ENTERPRISE_LOAD_PROFILE=1 bash scripts/agentd_enterprise_load_profile.sh --execute --driver "$AGENTD_LOAD_DRIVER" --evidence-dir "$AGENTD_ACCEPTANCE_DIR/ad-e7-load"`; cover tenant, project, room, Matrix event, queue, artifact/log, certification throughput, failure injection, test window, noisy neighbor, and budget dimensions.
- [ ] Run Palpo/Matrix ingress/replay and Robrix projections throughout the load profile; compare canonical project, task, lease, artifact, evidence, denial, capacity, and SLO ids without exposing prompt/transcript bytes.
- [ ] Lose a worker while leased; prove a new incarnation and higher fencing token recover the task and the old worker cannot publish any side effect.
- [ ] Lose the leader control-plane instance; prove a surviving member obtains a higher leadership term/fence, no accepted queue/lease/outbox/audit state is lost, and stale leader mutations conflict.
- [ ] Lose one entire zone; prove placement is not weakened, no accepted state is lost, replica/key policies hold, and recovery meets declared RPO/RTO.
- [ ] Block and restore Specify, KMS, object store, Matrix, and OpenFab independently; retain fail-closed admission, durable retry/replay, and no-fallback evidence.
- [ ] Execute a signed canary/zone rollout and forward rollback, queue/policy autoscaling, multi-region replica acknowledgement, tenant key rotation, retention expiry, legal-hold deletion denial/release, DR restore, and failback.
- [ ] Replay an old exact key transition after the key reaches `retired`, and an old exact replica acknowledgement after the replica reaches `available`; prove both return current state without regression. Reuse either identity with changed input and prove conflict.
- [ ] Race leader replacement against every enterprise mutation. Prove migration `0026` records the same term/fence in the mutation transaction and no stale leader writes after expiry or takeover.
- [ ] Replay old rollout observations, zone policies, retention policies, key transitions, and replica acknowledgements after newer state exists. Prove schema `27` returns current state for exact historical replay, rejects changed version reuse, and blocks direct update/delete of all audit histories.
- [ ] Compare `agentctl enterprise status`, exact task explain, dashboard, doctor, metrics, and immutable ledgers for leadership, zones, backlog, rollout, replicas, budget, failures, and SLOs.
- [ ] FSF-7 operator sign-off recorded.

## Unified Verification Sequence

- [ ] Ensure `ANTHROPIC_API_KEY` and `CLAUDE_API_KEY` are unset. Confirm no final smoke command or runtime matrix names Claude.
- [ ] Run `cargo fmt --all -- --check`.
- [ ] Run `cargo check --workspace --all-targets --all-features`.
- [ ] Run `cargo test --workspace --all-targets --all-features`.
- [ ] Run `cargo clippy --workspace --all-targets --all-features -- -D warnings`.
- [ ] Run `bash scripts/check.sh` and every guarded dry-run/preflight script before enabling real execution.
- [ ] Run the AD-E1 OCI security smoke, AD-E4 OpenFab checks, AD-E5 native runtime smoke, and `AGENTD_REAL_EXECUTE_RUNTIMES=codex,codex,codex,codex AGENTD_REAL_EXECUTE_SMOKE=1 bash scripts/agentd_real_execute_smoke.sh --execute`; retain Codex-only evidence.
- [ ] Start the dashboard against the final candidate and capture desktop/mobile browser screenshots; inspect enterprise band, run/runtime detail, overflow, auth failures, and nonblank live updates.
- [ ] Execute all AD-E1 through AD-E7 failure, rollback, restore, and cross-surface checks above in order; link immutable evidence digests before any status promotion.

## Final Decision

- [ ] Every evidence path is immutable and linked from this checklist.
- [ ] All rollback procedures were executed, not only reviewed.
- [ ] Required security, product, operations, and migration owners signed the final record.
- [ ] Only after the prior items pass, update roadmap/parity gates from candidate to accepted.
