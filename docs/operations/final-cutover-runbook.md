# Final Agent-Chat Cutover Runbook

- Status: AD-E6 code-complete candidate; commands remain unaccepted until the final manual run
- Runtime: agentd native process ownership only
- Legacy boundary: agent-chat is stopped and read-only during import; retained support is offline import only
- Real-agent policy: Codex only; unset `ANTHROPIC_API_KEY` and `CLAUDE_API_KEY`

This runbook performs a per-project forward-only ownership transfer. It never
rewinds a Matrix cursor, transfers a live lease to agent-chat, or treats a
legacy runtime address as durable identity.

## Evidence Setup

Create a new immutable evidence directory and record the exact repository,
binary, image, authority snapshot, policy, workflow, and database schema
digests. Do not place tokens, prompts, raw transcripts, or secret values in the
evidence directory.

```bash
export AGENTD_ACCEPTANCE_DIR="$(pwd)/.agentd/acceptance/$(date -u +%Y%m%dT%H%M%SZ)"
install -d -m 0700 "$AGENTD_ACCEPTANCE_DIR"
unset ANTHROPIC_API_KEY CLAUDE_API_KEY
git rev-parse HEAD >"$AGENTD_ACCEPTANCE_DIR/revision.txt"
```

Use one `CUTOVER_ID`, `PROJECT_REF`, database path, and agent-chat snapshot for
the complete procedure. Store command JSON output directly under the evidence
directory.

## Preflight And Backup

1. Stop agent-chat ingress and process launch. Keep its supported state files
   readable but do not permit writes or runtime control.
2. Confirm the project authority snapshot is current, the Matrix binding is
   exact, worker identities are healthy, and no revoked policy can dispatch.
3. Run structured diagnostics and resolve every `error` result.
4. Take an online SQLite backup and retain its generated digest manifest.

```bash
agentctl cutover doctor --db-path "$AGENTD_DB_PATH" \
  >"$AGENTD_ACCEPTANCE_DIR/doctor-preflight.json"
agentctl cutover backup --db-path "$AGENTD_DB_PATH" \
  --output "$AGENTD_ACCEPTANCE_DIR/agentd-pre-cutover.db" \
  >"$AGENTD_ACCEPTANCE_DIR/backup.json"
```

Verify local/team/fleet assets contain only native agentd startup commands:

- local: `deploy/local/io.agentd.plist`;
- team: `deploy/team/agentd.service` or `deploy/team/compose.yaml`;
- fleet handoff: `deploy/fleet/handoff.json` and the environment contract.

For an exact installation, render and record assets through
`agentctl cutover service-install`; keep credentials outside rendered files.

## Plan, Import, And Shadow

The source directory is an immutable copy of the stopped agent-chat state. Plan
and import must produce stable digest-only mappings. Replaying the same input is
idempotent; changed input under the same id must fail.

```bash
agentctl cutover plan --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  --project-ref "$PROJECT_REF" --legacy-root "$AGENT_CHAT_SNAPSHOT" \
  >"$AGENTD_ACCEPTANCE_DIR/plan.json"
agentctl cutover import --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  --legacy-root "$AGENT_CHAT_SNAPSHOT" \
  >"$AGENTD_ACCEPTANCE_DIR/import.json"
agentctl cutover shadow --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  --legacy-root "$AGENT_CHAT_SNAPSHOT" \
  >"$AGENTD_ACCEPTANCE_DIR/shadow.json"
```

Inspect all ID mappings and normalized routing, audience, task, graph, and
cursor decisions. Do not continue with drift, unsupported mutable state, or an
unresolved required parity row.

## Drain, Handoff, And Activate

Drain blocks new legacy work. Every accepted in-flight item must finish, be
cancelled with an audit reason, or be imported under a new agentd lease and
fencing epoch. Handoff is allowed only with zero legacy in-flight work and zero
shadow drift.

```bash
agentctl cutover drain --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  >"$AGENTD_ACCEPTANCE_DIR/drain.json"
agentctl cutover handoff --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  >"$AGENTD_ACCEPTANCE_DIR/handoff.json"
agentctl cutover activate --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  >"$AGENTD_ACCEPTANCE_DIR/activate.json"
agentctl cutover inspect --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  >"$AGENTD_ACCEPTANCE_DIR/inspect-active.json"
```

Start exactly one selected agentd service model. Check `/healthz`, structured
doctor output, worker pull/renewal, runtime recovery, Matrix replay, artifact
acknowledgement, and OpenFab outage behavior. Run the final Codex-only native
and execute smokes only during the unified acceptance pass.

## Rollback

Rollback is forward-only. Trigger it for duplicate accepted execution,
stale-fence mutation, authority/binding mismatch, cursor advancement without
durable acknowledgement, tenant isolation failure, evidence loss, or recovery
beyond RTO.

1. Stop new agentd ingress for the affected project.
2. Drain or explicitly cancel agentd-owned tasks under their current fences.
3. Preserve audit, mappings, artifacts, transcript references, and certification
   references.
4. Route only commands that were never accepted by agentd to a reviewed legacy
   contingency path; that path must consult the agentd deduplication ledger.

```bash
agentctl cutover rollback --db-path "$AGENTD_DB_PATH" --cutover-id "$CUTOVER_ID" \
  --reason "$ROLLBACK_REASON" \
  >"$AGENTD_ACCEPTANCE_DIR/rollback.json"
```

Never restore a database while agentd is listening. For disaster recovery,
stop the service, verify the backup manifest/digest/schema, restore atomically,
then rerun doctor before starting ingress:

```bash
agentctl cutover restore --db-path "$AGENTD_DB_PATH" \
  --backup "$AGENTD_ACCEPTANCE_DIR/agentd-pre-cutover.db" \
  --manifest "$AGENTD_ACCEPTANCE_DIR/agentd-pre-cutover.db.manifest.json"
agentctl cutover doctor --db-path "$AGENTD_DB_PATH" \
  >"$AGENTD_ACCEPTANCE_DIR/doctor-restored.json"
```

## Retirement And Sign-Off

After pilot drills and human authorization, record retirement through the
cutover ledger, remove agent-chat startup/service credentials, archive its
read-only snapshot under the retention policy, and deny every legacy runtime
control path. The offline importer remains the only compatibility surface.

Sign-off must link immutable evidence for shadow/import, rollback, worker loss,
authority outage, Matrix replay, certification outage, native runtime recovery,
backup/restore, service installation, and parity disposition. Only then may the
roadmap and capability map change from `candidate` to `accepted`.
