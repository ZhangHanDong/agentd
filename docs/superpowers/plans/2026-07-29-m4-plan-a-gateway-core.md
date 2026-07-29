# M4 Plan A — Matrix Gateway Core Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the Matrix ingress path crash-safe and replay-safe end to end — the gateway's inbound cursor becomes a daemon-owned durable record instead of nothing at all, one Matrix event becomes at most one accepted command through a single `BEGIN IMMEDIATE`, that command carries a canonical `command_id` under a unique room/project dedup constraint, and the command's run is created idempotently by the maintenance tick — so restart/replay produces zero duplicate accepted executions.

**Architecture:** Four durable records, one transaction, two maintenance sweeps. Migration `0028` adds `matrix_gateway_cursors`, the agentd-owned inbound sync cursor the remote bridge reads and advances *over HTTP only* (`GET`/`PUT /api/matrix/gateway/cursor`) — bridges never open the daemon database. Migration `0029` adds `matrix_commands`, keyed by a canonical `command_id` deterministically derived from `(room_id, event_id)`, with a partial unique index `(room_id, project_key, dedup_key)` over open rows that makes a second *open* command for the same room+project+payload a 409 instead of a second execution. `post_matrix_inbound_message` is then rewritten so event acceptance, command creation, the inbox message (under a **deterministic** message id derived from `command_id`), and the outbox relay event all land in one `BEGIN IMMEDIATE` or none of them do. Run creation is deliberately *not* in that transaction: it is driven off the durable command row by a maintenance sweep that creates a task graph under a deterministic graph id, mirroring `settle_node_executions`' shape and error discipline — which is also why the M3 I2 carry-over (nothing re-advances an active graph whose initial advance failed) is Task 1 rather than a follow-up ticket.

**Tech Stack:** Rust 2024, tokio, axum 0.7-style `Router`/extractors, sqlx + SQLite, serde/serde_json, `sha2`, `cargo nextest`, `tempfile`, `tower::ServiceExt::oneshot`, `http_body_util::BodyExt`.

## Global Constraints

- **Error classification:** `Invalid` → 400, `NotFound` → 404, `Conflict` → 409, and **only** `Unavailable` is retryable → 503. `crates/agentd-surface/src/control_plane_status.rs`'s `ControlPlaneErrorStatus` is the mapping pattern. On the routes this plan touches the mapping is by string convention and the helper already exists: `StoreError::Conflict` → `CoreError::Store("conflict: …")` → 409 and `StoreError::Invariant` → `CoreError::Invariant` → 400 are both handled by `task_error_response` (`crates/agentd-surface/src/http.rs:1697`). **Use `task_error_response`, never `agent_error_response`, on every route this plan adds or edits** — `agent_error_response` collapses `Conflict` onto 500.
- **Multi-statement mutations run inside `BEGIN IMMEDIATE`** with a `rows_affected` guard on every write. The pattern to copy verbatim is `message_repo::suppress_message_for_agent` (`crates/agentd-store/src/message_repo.rs:779-801`): `pool.acquire()`, `BEGIN IMMEDIATE`, call an `_in_transaction` helper, `COMMIT` on `Ok`, best-effort `ROLLBACK` on `Err`.
- **CAS / `record_version` discipline where a record can be concurrently written.** Every table this plan adds carries `record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0)`, and every update is `... SET record_version = record_version + 1 ... WHERE id = ? AND record_version = ?` with a `rows_affected() == 1` guard.
- **Never end a `Conflict` message with `"changed concurrently"`** unless you intend the task-graph retry semantics. `agent_chat_task_graph_repo::is_concurrent_write_conflict` (`crates/agentd-store/src/agent_chat_task_graph_repo.rs:1571`) matches on that exact suffix and its callers *retry* on it. This plan's CAS failures say `"record version mismatch"`.
- **Maintenance-tick errors are `tracing::warn!`-ed and swallowed** so one bad tick never stops the loop. `worker_fleet_tick` (`crates/agentd-bin/src/daemon.rs:129-152`) is the shape.
- **`agentd-surface` stays store-free.** It depends on `agentd-core` ports and its own `RunHost` trait only — never on `agentd-store`. Every surface type is a hand-mirrored struct; a new field on a store type that must reach the wire means editing the mirror in `agentd-surface/src/host.rs`, the mapping in `agentd-bin/src/host.rs`, **and** the in-memory fake in `agentd-surface/src/test_support.rs`.
- **Workers and bridges never open the daemon database — HTTP only.** This is load-bearing here: the Matrix gateway's bridge process is remote. `agentd-matrix` has no `agentd-store` dependency (`crates/agentd-matrix/Cargo.toml`) and must not gain one. Every new piece of gateway state reaches the bridge through an `AgentdHttpBackend` route.
- **Any schema change = a new migration bumping `schema_meta.version`,** with the `crates/agentd-store/tests/migration.rs` version assertions **and** the `crates/agentd-store/tests/operational_doctor.rs` schema-version assertion updated **in the same task**. Current version is **27**. This plan has exactly two: `0028_matrix_gateway_cursors.sql` → version **28** (Task 2) and `0029_matrix_commands.sql` → version **29** (Task 4). Do not fold them and do not add a third.
- **Parity status cells must NOT change without updating the contract tests in the same commit.** The suites are `crates/agentctl/tests/parity_cli.rs`, `crates/agentctl/tests/worktree_reconciliation_contract.rs`, and `crates/agentctl/tests/enterprise_project_authority_contract.rs`. `worktree_reconciliation_contract.rs:147` asserts `rows["matrix_bridge"][4] == "partial"`; `parity_cli.rs` asserts `matrix.status == "partial"` in every `parity_capability_map_records_p2*_matrix_*` test. **`matrix_bridge` and `remote_relay` both stay `partial` in this plan.** Task 7 only appends evidence text and adds one new assertion test.
- **Test gates are narrow.** Always a single `--test <name>` (or `--lib`) gate scoped to one package with `-p`. Never workspace-wide `cargo nextest run`. Never two `nextest` invocations concurrently. Avoid multi-package `-p a -p b` combinations (rebuild trap).
- **Before every commit:** `cargo fmt --all`, then `cargo clippy --all-targets -p <touched package> -- -D warnings`.

---

## Gap Analysis: what p236–p262 already cover, and what items 1+2 still lack

Read this before starting. It is why this plan has seven tasks and no room-mapping, trust, or bot-command task.

**The bridge↔daemon contract is complete and must not be re-litigated.** `POST /api/matrix/rooms`, `GET /api/matrix/rooms/:room_id`, `POST /api/matrix/inbound`, `GET /api/matrix/outbox`, `GET /api/matrix/outbox/cursor`, and `POST /api/matrix/outbox/ack` all exist (`crates/agentd-surface/src/http.rs:146-151`). Durable room trust/mapping lives in `matrix_bridge_rooms` (migration `0012`, `project_id` added by `0022`), the project↔room↔repo binding is a first-class record with `room_id` UNIQUE (migration `0025`), `[AGENTIGNORE]` suppression works, outbox echo filtering works (`http.rs:635`), and the whole `agentd-matrix` runtime — `AgentdHttpBackend`, `BridgeRuntime`, `MatrixClientBridgeTransport`, the opt-in `matrix-sdk-adapter` feature, puppet provisioning, preflight, bounded service assembly, bot command planning — is built. **Do not add a route to register rooms, and do not touch trust, ACLs, or command parsing: those are Plan B.**

### Gap 1 — the inbound cursor does not exist anywhere, durable or not

There are two directions and only one of them has a cursor.

*Outbox (agentd → Matrix)* is genuinely daemon-owned: `matrix_outbox_cursors` (migration `0021`) is a `bridge_id → last_seq` table with a `MAX(...)` monotone upsert (`crates/agentd-store/src/matrix_bridge_repo.rs:23-45`). The bridge also keeps `BridgeState { nextFromSeq }` in a JSON file (`crates/agentd-matrix/src/lib.rs:72-117`), but `run_bridge_once` floors it from the daemon (`lib.rs:3896-3897`, `state.next_from_seq.max(cursor)`), so the file is a cache and the DB is the floor. **This direction is fine. Leave it alone.**

*Inbound (Matrix → agentd)* has **no cursor at all**. `SdkMatrixClient::sync_once` builds `SyncSettings::new().timeout(...)` with **no `.token(...)`** (`lib.rs:3039-3046`), `MatrixClientSync` (`lib.rs:1658-1668`) has no `next_batch` field, and nothing anywhere persists a sync token — not in the daemon database, not even in the bridge's JSON state file, which holds only `nextFromSeq`. So a restarted gateway re-syncs from the homeserver's initial-sync limited timeline and **re-POSTs events it already delivered**. The spec's "`AgentdMatrixGateway`-owned durable cursor" is not "move the JSON file into the DB" — it is building the record for the first time. Tasks 2 and 3.

### Gap 2 — event idempotency is advisory, not atomic

`matrix_bridge_events` (migration `0012`) is a real durable table: `event_id` PRIMARY KEY, `INSERT … ON CONFLICT(event_id) DO NOTHING` (`matrix_bridge_repo.rs:164-194`). It survives restart. What it does **not** guarantee is one *effect* per event, because `post_matrix_inbound_message` (`crates/agentd-bin/src/host.rs:1957-2114`) is a read-then-side-effect-then-write sequence over the pool with no transaction:

1. `get_room` (read), 2. cutover phase + authority/lease fence check (read), 3. `get_event` — **the duplicate check** (read), 4. `get_room` again (read), 5. `post_group_message`/`post_direct_message`, which is itself **two** unrelated pool writes: `insert_direct_message` (`message_repo.rs:283-338`, message id from `generate_message_id()` — a fresh ULID on every call) **then** `append_relay_stream_event` (`crates/agentd-bin/src/host.rs:2509-2523`), 6. `record_event` (write).

Three concrete defects follow, and every one of them is reachable from the replay described in Gap 1:

- **Crash between 5 and 6** → the message exists and the event row does not. The replay re-runs step 3, sees no event, and inserts a **second message with a different ULID**. `ON CONFLICT(id) DO NOTHING` cannot help: the id is freshly generated. This is precisely the "duplicate accepted execution" the M4 exit criterion forbids.
- **Crash between the two writes inside step 5** → the inbox message exists but no outbox relay event was appended, so the Matrix side never sees the echo and the event is recorded as routed.
- **Two concurrent POSTs of the same `event_id`** → both pass step 3, both insert distinct messages, the second `record_event` is a silent `DO NOTHING`. One event, two messages.

The existing test `daemon_router_matrix_inbound_agent_dm_persists_source_metadata_and_dedupes_event` (`crates/agentd-bin/tests/daemon_http.rs:1852`) proves only the *sequential, non-crashing* duplicate path. Task 5.

### Gap 3 — there is no `command_id` and no room/project dedup constraint

`grep -rn "command_id\|commandId" crates/ --include='*.rs'` returns **nothing**. There is no command record at all: an inbound Matrix event becomes a chat message in `direct_messages`/`group_messages` and that is the end of it. Nothing is keyed such that "this Matrix event's command" can be named, deduplicated, or linked to a run. Tasks 4 and 6.

The unique constraint the spec asks for needs one design decision, made here so no task has to invent it: it is a **partial** unique index over *open* rows only —

```sql
CREATE UNIQUE INDEX idx_matrix_commands_open_room_project
    ON matrix_commands(room_id, project_key, dedup_key)
    WHERE status IN ('accepted', 'running');
```

A full unique index would make an ordinary chat room reject the second `ok` anyone types. Scoping it to `accepted`/`running` means the constraint says exactly one useful thing — *at most one open command per (room, project, payload)* — and a plain chat message is inserted directly as `settled`, so it never occupies the slot and existing chat behaviour is byte-identical. `project_key` is a `NOT NULL DEFAULT ''` column rather than a nullable `project_id`, because SQLite treats NULLs as distinct in a unique index and an unbound room would otherwise escape the constraint entirely.

### Gap 4 — nothing turns an inbound event into a run, and this plan must not make that Plan B's problem

Command *normalization* (deciding that `!run build` is a run request) is M4 item 3, i.e. Plan B. But the *handoff* is item 2 and belongs here. The resolution: `MatrixInboundMessageInput` gains an optional `runRequest` field that the daemon accepts and the bridge does not yet populate. With the field absent — every call made today — behaviour is unchanged and the command row is written `settled`. With it present, the command row is written `accepted` and Task 6's sweep creates the run. Plan B's only remaining job is to *populate* the field from a normalized command. Nothing in Plan A guesses at command syntax.

### Gap 5 — M3 carry-over I2 is now reachable from a Matrix room

`.superpowers/sdd/progress.md:116,120` records it: *"failed initial advance never retried (periodic advance sweep — do before M4 ships Matrix-driven graphs)"*. `advance_graph` has exactly one caller, `create_agent_chat_task_graph` (`crates/agentd-bin/src/host.rs:2340`), and `create_graph` only persists — `advance_graph` is what dispatches. So a create whose advance fails on a transient database error leaves an `active` graph with a `pending` root and **nothing in the system re-drives it**. Task 6 makes a Matrix room create graphs, which turns a latent bug into an operator-visible stranded room. Task 1 fixes it with an `advance_active_graphs` sweep in the maintenance tick, mirroring `settle_node_executions`' per-row isolation and error discipline. It is Task 1 and not a follow-up ticket because Task 6 depends on it: Task 6's sweep creates the graph, Task 1's sweep advances it.

### What this plan gets for free

`create_graph` returns `StoreError::Conflict("task graph already exists: <id>")` on a duplicate id (`agent_chat_task_graph_repo.rs:198-200`, `graph_already_exists`), which is exactly the idempotency primitive Task 6 needs — a replayed sweep hits the conflict, re-reads, and binds. `insert_direct_message`/`insert_group_message` accept an explicit `message_id` and are `ON CONFLICT(id) DO NOTHING`, so a deterministic id makes the message insert replay-safe. `task_error_response` already classifies `Conflict` → 409. `sha2 = "0.10"` is already a dependency of both `agentd-store` and `agentd-matrix`.

---

## Non-Goals (explicitly out of scope for this plan)

M4 is sliced the way M2 and M3 were. **Plan A is items 1 and 2 only.**

- **M4 item 3 — trusted inviter, ignored sender, appservice loop suppression, command normalization: Plan B.** Nothing here changes `matrix_bridge_rooms.trusted`, the `[AGENTIGNORE]` rule, `MatrixPuppetDirectory` loop suppression, or `MatrixBotCommandPlan`. Plan A's `dedup_key` is a hash of a *minimally* normalized body (trim + ASCII-lowercase + internal-whitespace collapse) and says so in a comment; Plan B replaces the normalizer and the `dedup_key` derivation together.
- **M4 item 4 — attachment ingest as content-addressed project input: Plan B.**
- **M4 item 5 — Robrix project/run/task/artifact/approval/evidence views: Plan C.**
- **Populating `runRequest` from a parsed bang command.** Plan A defines and honours the field; Plan B fills it in.
- **Persisting the Matrix sync token from inside `SdkMatrixClient`.** Task 3 gives the bridge the durable cursor record and the HTTP methods to read and advance it, and wires `BridgeRuntime` to carry it. Threading the token into `SyncSettings::token(...)` requires the `matrix-sdk-adapter` feature and a real homeserver to prove, which is Plan B's real-homeserver evidence slice. Task 3's tests use fake clients only, exactly as p250–p254 do.
- **Cancelling a command's already-created graph.** No `DurableSchedulerPort::cancel` exists (M3 Plan C non-goal, still open). A settled command's graph runs to completion.
- **Multi-gateway cursor fan-out.** `AgentdHttpBackend` hardcodes `bridgeId=matrix-bridge` (`crates/agentd-matrix/src/lib.rs:4072,4080`); Task 3 introduces `gateway_id` as a first-class parameter for the *new* cursor but does not retrofit the outbox cursor. Follow-up ticket.
- **New chat routes, message shapes, or task/task-graph CRUD.** All at parity already.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/agentd-store/src/agent_chat_task_graph_repo.rs` | add `advance_active_graphs` (the I2 sweep) | 1 |
| `crates/agentd-bin/src/daemon.rs` | wire both new sweeps into `worker_fleet_tick` | 1, 6 |
| `crates/agentd-store/migrations/0028_matrix_gateway_cursors.sql` | daemon-owned inbound cursor table, version → 28 | 2 |
| `crates/agentd-store/migrations/0029_matrix_commands.sql` | canonical command record + dedup index, version → 29 | 4 |
| `crates/agentd-store/src/matrix_bridge_repo.rs` | gateway cursor CAS; command record; connection-scoped event insert; command→run binding | 2, 4, 5, 6 |
| `crates/agentd-store/src/message_repo.rs` | connection-scoped `_on` variants of the two message inserts | 5 |
| `crates/agentd-store/src/relay_repo.rs` | connection-scoped `_on` variant of the relay-stream append | 5 |
| `crates/agentd-surface/src/host.rs` | mirrored cursor/command types + two new `RunHost` methods + `runRequest` on the inbound input | 3, 5 |
| `crates/agentd-surface/src/http.rs` | `GET`/`PUT /api/matrix/gateway/cursor` | 3 |
| `crates/agentd-surface/src/test_support.rs` | in-memory fake for the new `RunHost` methods | 3 |
| `crates/agentd-bin/src/host.rs` | production impl of the new methods; the transactional inbound rewrite | 3, 5 |
| `crates/agentd-matrix/src/lib.rs` | `AgentdHttpBackend` cursor methods; `AgentdBridgeBackend` sync-cursor hooks; `BridgeRuntime` carries the cursor | 3 |
| `docs/parity/agent-chat-capability-map.md`, `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md` | p263/p264 evidence | 7 |

---

## Task 1: Periodic advance of active task graphs (M3 carry-over I2)

**Files:**
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs` (add `advance_active_graphs` next to `settle_node_executions`, ~line 949)
- Modify: `crates/agentd-bin/src/daemon.rs:142-149` (inside `worker_fleet_tick`)
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs` (append)

**Interfaces:**
- Consumes: existing `agent_chat_task_graph_repo::{create_graph, advance_graph, list_graphs, CreateAgentChatTaskGraph, AgentChatTaskGraphNodeInput}`.
- Produces: `pub async fn advance_active_graphs(pool: &SqlitePool) -> Result<u64, StoreError>` — returns how many `active` graphs were successfully advanced. Task 6 relies on this existing and on it being called from `worker_fleet_tick`.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
#[tokio::test]
async fn advance_active_graphs_redrives_a_graph_whose_initial_advance_never_ran() {
    let (store, _dir) = open_store().await;

    // `create_graph` only persists; `advance_graph` is what dispatches. A
    // create whose advance failed on a transient database error leaves exactly
    // this state, and before this sweep nothing re-drove it.
    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    let created = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_stranded".to_string()),
            owner: "orchestrator".to_string(),
            label: "Stranded graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    assert_eq!(created.status, "active");
    assert_eq!(created.nodes["a"].status, "pending");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        0
    );

    let advanced = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("advance active graphs");
    assert_eq!(advanced, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_stranded")
        .await
        .expect("get graph")
        .expect("graph exists");
    assert_eq!(graph.nodes["a"].status, "dispatched");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        1
    );
}

#[tokio::test]
async fn advance_active_graphs_is_idempotent_and_skips_settled_graphs() {
    let (store, _dir) = open_store().await;

    let mut nodes = BTreeMap::new();
    nodes.insert("a".to_string(), node("codex-a", "Do A", &[]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_once".to_string()),
            owner: "orchestrator".to_string(),
            label: "Once".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let first = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("first sweep");
    assert_eq!(first, 1);
    let second = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("second sweep");
    assert_eq!(second, 1, "an already-dispatched active graph re-advances cleanly");
    // The dispatch itself must not be repeated: one message, not two.
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        1
    );

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_once")
        .await
        .expect("delete graph");
    let third = agent_chat_task_graph_repo::advance_active_graphs(store.pool())
        .await
        .expect("third sweep");
    assert_eq!(third, 0, "a cancelled graph is not active and is skipped");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs -E 'test(advance_active_graphs)'`
Expected: FAIL — `no function or associated item named 'advance_active_graphs' found`.

- [ ] **Step 3: Write minimal implementation**

Add to `crates/agentd-store/src/agent_chat_task_graph_repo.rs`, immediately above `pub async fn settle_node_executions`:

```rust
/// Re-drive every `active` task graph.
///
/// `advance_graph` has exactly one caller — graph creation — so a create whose
/// advance failed on a transient database error strands an `active` graph with
/// a `pending` root that nothing else re-drives. Advancing is idempotent (a
/// node already `dispatched` is not re-dispatched), so an unconditional sweep
/// is the whole repair.
///
/// Returns the number of graphs advanced. One graph's failure is isolated and
/// logged, never propagated: this runs on the maintenance tick, where a single
/// poisoned graph must not stop the sweep or the loop.
pub async fn advance_active_graphs(pool: &SqlitePool) -> Result<u64, StoreError> {
    let graphs = list_graphs(pool, Some("active")).await?;
    let mut advanced = 0_u64;
    for graph in graphs {
        match advance_graph(pool, &graph.id).await {
            Ok(Some(_)) => advanced += 1,
            // Deleted between the listing and the advance; nothing to repair.
            Ok(None) => {}
            Err(error) => {
                tracing::warn!(
                    graph_id = graph.id.as_str(),
                    %error,
                    "re-advancing active task graph failed this tick"
                );
            }
        }
    }
    Ok(advanced)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs -E 'test(advance_active_graphs)'`
Expected: PASS (2 tests).

- [ ] **Step 5: Wire the sweep into the maintenance tick**

In `crates/agentd-bin/src/daemon.rs`, inside `worker_fleet_tick`, immediately after the existing `settle_node_executions` block (which ends at line 149) and before `agent_registry_tick`:

```rust
    // Settlement moves nodes to terminal states; advancing is what unlocks the
    // downstream ones and re-drives any graph whose creation-time advance
    // failed. Order matters: advance after settle, so a node settled this tick
    // unlocks its dependants in the same tick.
    if let Err(error) =
        agentd_store::agent_chat_task_graph_repo::advance_active_graphs(native_worker.store().pool())
            .await
    {
        tracing::warn!(%error, "re-advancing active task graphs failed this tick");
    }
```

- [ ] **Step 6: Verify the daemon still builds and its suite passes**

Run: `cargo nextest run -p agentd-bin --test m3_coordination_e2e`
Expected: PASS, no regressions.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
cargo clippy --all-targets -p agentd-bin -- -D warnings
git add crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs \
        crates/agentd-bin/src/daemon.rs
git commit -m "fix(task-graph): re-advance active graphs from the maintenance tick"
```

---

## Task 2: Migration 0028 — daemon-owned Matrix gateway cursor

**Files:**
- Create: `crates/agentd-store/migrations/0028_matrix_gateway_cursors.sql`
- Modify: `crates/agentd-store/src/matrix_bridge_repo.rs` (append)
- Modify: `crates/agentd-store/tests/migration.rs` (every `assert_eq!(version, "27")` → `"28"`)
- Modify: `crates/agentd-store/tests/operational_doctor.rs:23` (`report.schema_version, 27` → `28`)
- Test: `crates/agentd-store/tests/matrix_bridge.rs` (append)

**Interfaces:**
- Consumes: `crate::error::StoreError`, `crate::util::now_unix`, and the existing private `required`/`clean_opt` helpers in `matrix_bridge_repo.rs:222-231`.
- Produces:
  - `pub struct MatrixGatewayCursorRecord { pub gateway_id: String, pub sync_token: Option<String>, pub last_event_id: Option<String>, pub record_version: i64, pub created_at: i64, pub updated_at: i64 }`
  - `pub struct MatrixGatewayCursorInput { pub gateway_id: String, pub sync_token: Option<String>, pub last_event_id: Option<String>, pub expected_version: Option<i64> }`
  - `pub async fn get_gateway_cursor(pool: &SqlitePool, gateway_id: &str) -> Result<Option<MatrixGatewayCursorRecord>, StoreError>`
  - `pub async fn advance_gateway_cursor(pool: &SqlitePool, input: MatrixGatewayCursorInput) -> Result<MatrixGatewayCursorRecord, StoreError>`

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/matrix_bridge.rs`:

```rust
#[tokio::test]
async fn matrix_gateway_cursor_is_created_then_advanced_under_compare_and_set() {
    let (store, _dir) = open_temp().await;

    assert!(
        matrix_bridge_repo::get_gateway_cursor(store.pool(), "gateway-1")
            .await
            .expect("get missing cursor")
            .is_none()
    );

    let created = matrix_bridge_repo::advance_gateway_cursor(
        store.pool(),
        matrix_bridge_repo::MatrixGatewayCursorInput {
            gateway_id: text("gateway-1"),
            sync_token: Some(text("s_batch_1")),
            last_event_id: Some(text("$event-1")),
            expected_version: None,
        },
    )
    .await
    .expect("create cursor");
    assert_eq!(created.record_version, 1);
    assert_eq!(created.sync_token.as_deref(), Some("s_batch_1"));

    let advanced = matrix_bridge_repo::advance_gateway_cursor(
        store.pool(),
        matrix_bridge_repo::MatrixGatewayCursorInput {
            gateway_id: text("gateway-1"),
            sync_token: Some(text("s_batch_2")),
            last_event_id: Some(text("$event-2")),
            expected_version: Some(1),
        },
    )
    .await
    .expect("advance cursor");
    assert_eq!(advanced.record_version, 2);
    assert_eq!(advanced.sync_token.as_deref(), Some("s_batch_2"));
    assert_eq!(advanced.last_event_id.as_deref(), Some("$event-2"));

    // The cursor survives a reopen: it lives in the daemon database, not in a
    // JSON file next to the bridge binary.
    let loaded = matrix_bridge_repo::get_gateway_cursor(store.pool(), "gateway-1")
        .await
        .expect("get cursor")
        .expect("cursor exists");
    assert_eq!(loaded.record_version, 2);
    assert_eq!(loaded.sync_token.as_deref(), Some("s_batch_2"));
}

#[tokio::test]
async fn matrix_gateway_cursor_rejects_a_stale_or_missing_version() {
    let (store, _dir) = open_temp().await;

    matrix_bridge_repo::advance_gateway_cursor(
        store.pool(),
        matrix_bridge_repo::MatrixGatewayCursorInput {
            gateway_id: text("gateway-1"),
            sync_token: Some(text("s_batch_1")),
            last_event_id: None,
            expected_version: None,
        },
    )
    .await
    .expect("create cursor");

    let stale = matrix_bridge_repo::advance_gateway_cursor(
        store.pool(),
        matrix_bridge_repo::MatrixGatewayCursorInput {
            gateway_id: text("gateway-1"),
            sync_token: Some(text("s_batch_stale")),
            last_event_id: None,
            expected_version: Some(7),
        },
    )
    .await
    .expect_err("stale version must conflict");
    let message = stale.to_string();
    assert!(message.contains("record version mismatch"), "{message}");
    // Must NOT trip the task-graph retry wrapper's sentinel.
    assert!(!message.ends_with("changed concurrently"), "{message}");

    let recreate = matrix_bridge_repo::advance_gateway_cursor(
        store.pool(),
        matrix_bridge_repo::MatrixGatewayCursorInput {
            gateway_id: text("gateway-1"),
            sync_token: Some(text("s_batch_clobber")),
            last_event_id: None,
            expected_version: None,
        },
    )
    .await
    .expect_err("a versionless write must not clobber an existing cursor");
    assert!(recreate.to_string().contains("record version mismatch"));

    let loaded = matrix_bridge_repo::get_gateway_cursor(store.pool(), "gateway-1")
        .await
        .expect("get cursor")
        .expect("cursor exists");
    assert_eq!(loaded.sync_token.as_deref(), Some("s_batch_1"));
    assert_eq!(loaded.record_version, 1);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-store --test matrix_bridge -E 'test(gateway_cursor)'`
Expected: FAIL — `no function or associated item named 'advance_gateway_cursor'`.

- [ ] **Step 3: Write the migration**

Create `crates/agentd-store/migrations/0028_matrix_gateway_cursors.sql`:

```sql
-- M4 Plan A: the AgentdMatrixGateway-owned durable inbound cursor.
--
-- Before this table there was no inbound cursor anywhere: `SdkMatrixClient`
-- syncs with no token and the bridge's JSON state file holds only the outbox
-- sequence, so a restarted gateway re-delivered whatever the homeserver's
-- initial sync returned. The daemon owns this record; the remote bridge reads
-- and advances it over HTTP and never opens this database.
CREATE TABLE matrix_gateway_cursors (
    gateway_id     TEXT PRIMARY KEY CHECK (length(trim(gateway_id)) > 0),
    sync_token     TEXT,
    last_event_id  TEXT,
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

UPDATE schema_meta SET value = '28' WHERE key = 'version';
```

- [ ] **Step 4: Write the repo functions**

Append to `crates/agentd-store/src/matrix_bridge_repo.rs`, above the private helpers at line 196:

```rust
/// Durable, agentd-owned Matrix gateway inbound cursor.
#[derive(Debug, Clone)]
pub struct MatrixGatewayCursorRecord {
    pub gateway_id: String,
    pub sync_token: Option<String>,
    pub last_event_id: Option<String>,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One cursor advance. `expected_version` is the compare-and-set predicate:
/// `None` creates the row and fails if it already exists; `Some(v)` updates
/// only the row still at `v`.
#[derive(Debug, Clone)]
pub struct MatrixGatewayCursorInput {
    pub gateway_id: String,
    pub sync_token: Option<String>,
    pub last_event_id: Option<String>,
    pub expected_version: Option<i64>,
}

pub async fn get_gateway_cursor(
    pool: &SqlitePool,
    gateway_id: &str,
) -> Result<Option<MatrixGatewayCursorRecord>, StoreError> {
    let gateway_id = required(gateway_id.to_string(), "matrix gateway id required")?;
    let row = sqlx::query(
        "SELECT gateway_id, sync_token, last_event_id, record_version, created_at, updated_at \
         FROM matrix_gateway_cursors WHERE gateway_id = ?",
    )
    .bind(gateway_id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|row| row_to_gateway_cursor(&row)))
}

/// Create or advance one gateway cursor under compare-and-set.
///
/// # Errors
/// [`StoreError::Invariant`] when the gateway id is blank;
/// [`StoreError::Conflict`] when `expected_version` does not match the stored
/// `record_version` (including a `None` against an existing row). The message
/// deliberately does not end in `"changed concurrently"`: that suffix is the
/// task-graph retry sentinel and must not be borrowed here.
pub async fn advance_gateway_cursor(
    pool: &SqlitePool,
    input: MatrixGatewayCursorInput,
) -> Result<MatrixGatewayCursorRecord, StoreError> {
    let gateway_id = required(input.gateway_id, "matrix gateway id required")?;
    let sync_token = clean_opt(input.sync_token);
    let last_event_id = clean_opt(input.last_event_id);
    let now = now_unix();

    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = advance_gateway_cursor_in_transaction(
        &mut connection,
        &gateway_id,
        sync_token.as_deref(),
        last_event_id.as_deref(),
        input.expected_version,
        now,
    )
    .await;
    match result {
        Ok(record) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(record)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn advance_gateway_cursor_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    gateway_id: &str,
    sync_token: Option<&str>,
    last_event_id: Option<&str>,
    expected_version: Option<i64>,
    now: i64,
) -> Result<MatrixGatewayCursorRecord, StoreError> {
    match expected_version {
        None => {
            let inserted = sqlx::query(
                "INSERT INTO matrix_gateway_cursors \
                 (gateway_id, sync_token, last_event_id, record_version, created_at, updated_at) \
                 VALUES (?, ?, ?, 1, ?, ?) \
                 ON CONFLICT(gateway_id) DO NOTHING",
            )
            .bind(gateway_id)
            .bind(sync_token)
            .bind(last_event_id)
            .bind(now)
            .bind(now)
            .execute(&mut *connection)
            .await?;
            if inserted.rows_affected() != 1 {
                return Err(gateway_cursor_version_mismatch(gateway_id));
            }
        }
        Some(version) => {
            let updated = sqlx::query(
                "UPDATE matrix_gateway_cursors \
                 SET sync_token = ?, last_event_id = ?, \
                     record_version = record_version + 1, updated_at = ? \
                 WHERE gateway_id = ? AND record_version = ?",
            )
            .bind(sync_token)
            .bind(last_event_id)
            .bind(now)
            .bind(gateway_id)
            .bind(version)
            .execute(&mut *connection)
            .await?;
            if updated.rows_affected() != 1 {
                return Err(gateway_cursor_version_mismatch(gateway_id));
            }
        }
    }

    let row = sqlx::query(
        "SELECT gateway_id, sync_token, last_event_id, record_version, created_at, updated_at \
         FROM matrix_gateway_cursors WHERE gateway_id = ?",
    )
    .bind(gateway_id)
    .fetch_optional(&mut *connection)
    .await?
    .ok_or_else(|| {
        StoreError::Invariant(format!("matrix gateway cursor '{gateway_id}' is missing"))
    })?;
    Ok(row_to_gateway_cursor(&row))
}

fn gateway_cursor_version_mismatch(gateway_id: &str) -> StoreError {
    StoreError::Conflict(format!(
        "matrix gateway cursor '{gateway_id}' record version mismatch"
    ))
}

fn row_to_gateway_cursor(row: &sqlx::sqlite::SqliteRow) -> MatrixGatewayCursorRecord {
    MatrixGatewayCursorRecord {
        gateway_id: row.get("gateway_id"),
        sync_token: row.get("sync_token"),
        last_event_id: row.get("last_event_id"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}
```

- [ ] **Step 5: Sweep the schema-version assertions**

Run: `sed -i '' 's/assert_eq!(version, "27")/assert_eq!(version, "28")/g' crates/agentd-store/tests/migration.rs`
Then in `crates/agentd-store/tests/operational_doctor.rs:23` change `assert_eq!(report.schema_version, 27);` to `assert_eq!(report.schema_version, 28);`.

Verify nothing was missed: `grep -rn '"27"\|schema_version, 27' crates/agentd-store/tests/` must return no hits.

- [ ] **Step 6: Add the migration table assertion**

In `crates/agentd-store/tests/migration.rs`, append:

```rust
#[tokio::test]
async fn migration_creates_matrix_gateway_cursor_table() {
    let (store, _dir) = open_temp().await;
    let name: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'matrix_gateway_cursors'",
    )
    .fetch_optional(store.pool())
    .await
    .expect("query sqlite_master");
    assert_eq!(name.as_deref(), Some("matrix_gateway_cursors"));

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version");
    assert_eq!(version, "28");
}
```

- [ ] **Step 7: Run the tests**

Run: `cargo nextest run -p agentd-store --test matrix_bridge`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test migration`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test operational_doctor`
Expected: PASS.

- [ ] **Step 8: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
git add crates/agentd-store/migrations/0028_matrix_gateway_cursors.sql \
        crates/agentd-store/src/matrix_bridge_repo.rs \
        crates/agentd-store/tests/matrix_bridge.rs \
        crates/agentd-store/tests/migration.rs \
        crates/agentd-store/tests/operational_doctor.rs
git commit -m "feat(matrix): add the daemon-owned Matrix gateway inbound cursor"
```

---

## Task 3: Gateway cursor over HTTP, and the bridge reads it

**Files:**
- Modify: `crates/agentd-surface/src/host.rs` (mirrored types + two `RunHost` methods, near the Matrix block at lines 286-375 and 1157-1190)
- Modify: `crates/agentd-surface/src/http.rs` (routes at line 151, handlers near line 662)
- Modify: `crates/agentd-surface/src/test_support.rs` (fake impl next to the Matrix fakes at line 855)
- Modify: `crates/agentd-bin/src/host.rs` (production impl next to `get_matrix_bridge_room`, line 1949)
- Modify: `crates/agentd-matrix/src/lib.rs` (`AgentdBridgeBackend` default hooks near line 3199; `AgentdHttpBackend` methods near line 4068; `BridgeRuntime::run_once` near line 3276)
- Test: `crates/agentd-bin/tests/daemon_http.rs` (append), `crates/agentd-matrix/tests/http_backend.rs` (append)

**Interfaces:**
- Consumes: Task 2's `matrix_bridge_repo::{get_gateway_cursor, advance_gateway_cursor, MatrixGatewayCursorInput, MatrixGatewayCursorRecord}`.
- Produces:
  - Surface types `MatrixGatewayCursorRecord { gateway_id, sync_token, last_event_id, record_version, created_at, updated_at }` (serialized `gatewayId`, `syncToken`, `lastEventId`, `recordVersion`, `created_at`, `updated_at`) and `MatrixGatewayCursorInput { gateway_id, sync_token, last_event_id, expected_version }` (deserialized `gatewayId`, `syncToken`, `lastEventId`, `expectedVersion`).
  - `RunHost::matrix_gateway_cursor(&self, gateway_id: &str) -> Result<Option<MatrixGatewayCursorRecord>, CoreError>` and `RunHost::advance_matrix_gateway_cursor(&self, input: MatrixGatewayCursorInput) -> Result<MatrixGatewayCursorRecord, CoreError>`.
  - Routes `GET /api/matrix/gateway/cursor?gatewayId=<id>` → `200` with the record or `404`; `PUT /api/matrix/gateway/cursor` → `200` with the record, `409` on version mismatch, `400` on a blank id.
  - `AgentdBridgeBackend::gateway_cursor(&mut self) -> Result<Option<(Option<String>, i64)>, BridgeError>` (default `Ok(None)`) and `AgentdBridgeBackend::advance_gateway_cursor(&mut self, sync_token: Option<&str>, last_event_id: Option<&str>, expected_version: Option<i64>) -> Result<i64, BridgeError>` (default `Ok(0)`), returning the new `record_version`.
  - `BridgeState` gains `sync_token: Option<String>` and `cursor_version: Option<i64>`; `BridgeState::new` keeps its existing one-argument signature and the new fields default to `None`.

- [ ] **Step 1: Write the failing HTTP test**

Append to `crates/agentd-bin/tests/daemon_http.rs`:

```rust
#[tokio::test]
async fn daemon_router_matrix_gateway_cursor_round_trips_and_fences_stale_writes() {
    let (app, _dir) = empty_router().await;

    let (missing_status, missing_body) =
        get(app.clone(), "/api/matrix/gateway/cursor?gatewayId=gateway-1").await;
    assert_eq!(missing_status, StatusCode::NOT_FOUND, "body: {missing_body}");

    let (created_status, created_body) = put(
        app.clone(),
        "/api/matrix/gateway/cursor",
        serde_json::json!({
            "gatewayId": "gateway-1",
            "syncToken": "s_batch_1",
            "lastEventId": "$event-1"
        }),
    )
    .await;
    assert_eq!(created_status, StatusCode::OK, "body: {created_body}");
    let created: serde_json::Value = serde_json::from_str(&created_body).expect("created json");
    assert_eq!(created["cursor"]["gatewayId"], "gateway-1");
    assert_eq!(created["cursor"]["syncToken"], "s_batch_1");
    assert_eq!(created["cursor"]["recordVersion"], 1);

    let (advanced_status, advanced_body) = put(
        app.clone(),
        "/api/matrix/gateway/cursor",
        serde_json::json!({
            "gatewayId": "gateway-1",
            "syncToken": "s_batch_2",
            "lastEventId": "$event-2",
            "expectedVersion": 1
        }),
    )
    .await;
    assert_eq!(advanced_status, StatusCode::OK, "body: {advanced_body}");
    let advanced: serde_json::Value = serde_json::from_str(&advanced_body).expect("advanced json");
    assert_eq!(advanced["cursor"]["recordVersion"], 2);

    let (stale_status, stale_body) = put(
        app.clone(),
        "/api/matrix/gateway/cursor",
        serde_json::json!({
            "gatewayId": "gateway-1",
            "syncToken": "s_batch_stale",
            "expectedVersion": 1
        }),
    )
    .await;
    assert_eq!(stale_status, StatusCode::CONFLICT, "body: {stale_body}");

    let (read_status, read_body) =
        get(app, "/api/matrix/gateway/cursor?gatewayId=gateway-1").await;
    assert_eq!(read_status, StatusCode::OK, "body: {read_body}");
    let read: serde_json::Value = serde_json::from_str(&read_body).expect("read json");
    assert_eq!(read["cursor"]["syncToken"], "s_batch_2");
    assert_eq!(read["cursor"]["recordVersion"], 2);
}
```

`daemon_http.rs` has `get` and `post` helpers but no `put`. Add one next to `post` (line 203):

```rust
async fn put(app: Router, uri: &str, body: serde_json::Value) -> (StatusCode, String) {
    let request = Request::builder()
        .method("PUT")
        .uri(uri)
        .header("content-type", "application/json")
        .header("authorization", format!("Bearer {OPERATOR_TOKEN}"))
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("collect body")
        .to_bytes();
    (status, String::from_utf8(bytes.to_vec()).expect("utf8"))
}
```

Read the existing `post` helper first and copy its exact authorization header construction — if it names the operator token differently, use that name here rather than `OPERATOR_TOKEN`.

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-bin --test daemon_http -E 'test(matrix_gateway_cursor)'`
Expected: FAIL — the `GET` returns `404` from axum's fallback and the `PUT` returns `405`/`404`, so the `created_status == OK` assertion fails.

- [ ] **Step 3: Add the surface types and `RunHost` methods**

In `crates/agentd-surface/src/host.rs`, after `MatrixOutboxCursorInput` (line 361):

```rust
/// Durable, agentd-owned Matrix gateway inbound cursor, as seen on the wire.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixGatewayCursorRecord {
    #[serde(rename = "gatewayId")]
    pub gateway_id: String,
    #[serde(rename = "syncToken")]
    pub sync_token: Option<String>,
    #[serde(rename = "lastEventId")]
    pub last_event_id: Option<String>,
    #[serde(rename = "recordVersion")]
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

/// One gateway cursor advance. `expected_version` is the compare-and-set
/// predicate: absent creates the cursor, present updates only that version.
#[derive(Debug, Clone, Deserialize)]
pub struct MatrixGatewayCursorInput {
    #[serde(rename = "gatewayId", alias = "gateway_id")]
    pub gateway_id: String,
    #[serde(default, rename = "syncToken", alias = "sync_token")]
    pub sync_token: Option<String>,
    #[serde(default, rename = "lastEventId", alias = "last_event_id")]
    pub last_event_id: Option<String>,
    #[serde(default, rename = "expectedVersion", alias = "expected_version")]
    pub expected_version: Option<i64>,
}
```

And in the `RunHost` trait, after `matrix_outbox_cursor` (line 1190):

```rust
    /// Read the durable inbound cursor for one Matrix gateway.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure.
    async fn matrix_gateway_cursor(
        &self,
        gateway_id: &str,
    ) -> Result<Option<MatrixGatewayCursorRecord>, CoreError>;

    /// Create or advance one Matrix gateway inbound cursor under CAS.
    ///
    /// # Errors
    /// [`CoreError`] on a store failure or a version mismatch.
    async fn advance_matrix_gateway_cursor(
        &self,
        input: MatrixGatewayCursorInput,
    ) -> Result<MatrixGatewayCursorRecord, CoreError>;
```

- [ ] **Step 4: Add the routes and handlers**

In `crates/agentd-surface/src/http.rs`, extend the import at line 40 with `MatrixGatewayCursorInput, MatrixGatewayCursorRecord`, add the routes after line 151:

```rust
        .route(
            "/api/matrix/gateway/cursor",
            get(matrix_gateway_cursor).put(put_matrix_gateway_cursor),
        )
```

Add the query struct next to `MatrixCursorQuery` (line 331):

```rust
#[derive(Debug, Deserialize)]
struct MatrixGatewayCursorQuery {
    #[serde(rename = "gatewayId", alias = "gateway_id")]
    gateway_id: String,
}
```

And the handlers after `matrix_outbox_cursor` (line 674):

```rust
async fn matrix_gateway_cursor(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<MatrixGatewayCursorQuery>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.matrix_gateway_cursor(&query.gateway_id).await {
        Ok(Some(cursor)) => Json(json!({ "cursor": cursor })).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "matrix gateway cursor not found" })),
        )
            .into_response(),
        // `task_error_response`, not `agent_error_response`: a CAS conflict
        // must surface as 409, and only 503 is retryable.
        Err(error) => task_error_response(error),
    }
}

async fn put_matrix_gateway_cursor(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<MatrixGatewayCursorInput>,
) -> Response {
    if let Err(err) = require_operator_bearer(&state.auth, &headers) {
        return err.into_response();
    }
    match state.host.advance_matrix_gateway_cursor(req).await {
        Ok(cursor) => Json(json!({ "ok": true, "cursor": cursor })).into_response(),
        Err(error) => task_error_response(error),
    }
}
```

Ensure `put` is in the `axum::routing` import list at the top of the file (add it if only `get`/`post`/`patch`/`delete` are imported).

- [ ] **Step 5: Implement the production host**

In `crates/agentd-bin/src/host.rs`, after `get_matrix_bridge_room` (line 1955), using the crate's existing surface-type aliases (mirror how `SurfaceMatrixBridgeRoomRecord` is aliased at the top of the file, and add `MatrixGatewayCursorInput as SurfaceMatrixGatewayCursorInput, MatrixGatewayCursorRecord as SurfaceMatrixGatewayCursorRecord` to that import list):

```rust
    async fn matrix_gateway_cursor(
        &self,
        gateway_id: &str,
    ) -> Result<Option<SurfaceMatrixGatewayCursorRecord>, CoreError> {
        let cursor = matrix_bridge_repo::get_gateway_cursor(self.store.pool(), gateway_id)
            .await
            .map_err(core_from_store_error)?;
        Ok(cursor.map(surface_matrix_gateway_cursor))
    }

    async fn advance_matrix_gateway_cursor(
        &self,
        input: SurfaceMatrixGatewayCursorInput,
    ) -> Result<SurfaceMatrixGatewayCursorRecord, CoreError> {
        let cursor = matrix_bridge_repo::advance_gateway_cursor(
            self.store.pool(),
            matrix_bridge_repo::MatrixGatewayCursorInput {
                gateway_id: input.gateway_id,
                sync_token: input.sync_token,
                last_event_id: input.last_event_id,
                expected_version: input.expected_version,
            },
        )
        .await
        .map_err(core_from_store_error)?;
        Ok(surface_matrix_gateway_cursor(cursor))
    }
```

And next to `surface_matrix_bridge_room` (find it with `grep -n "fn surface_matrix_bridge_room" crates/agentd-bin/src/host.rs`):

```rust
fn surface_matrix_gateway_cursor(
    cursor: matrix_bridge_repo::MatrixGatewayCursorRecord,
) -> SurfaceMatrixGatewayCursorRecord {
    SurfaceMatrixGatewayCursorRecord {
        gateway_id: cursor.gateway_id,
        sync_token: cursor.sync_token,
        last_event_id: cursor.last_event_id,
        record_version: cursor.record_version,
        created_at: cursor.created_at,
        updated_at: cursor.updated_at,
    }
}
```

- [ ] **Step 6: Implement the in-memory fake**

In `crates/agentd-surface/src/test_support.rs`, add a field to the fake host struct next to `matrix_outbox_cursors` (find it with `grep -n "matrix_outbox_cursors" crates/agentd-surface/src/test_support.rs`):

```rust
    matrix_gateway_cursors: Mutex<BTreeMap<String, MatrixGatewayCursorRecord>>,
```

initialize it as `Mutex::new(BTreeMap::new())` in the constructor (match how `matrix_outbox_cursors` is initialized; if the struct derives `Default`, no constructor edit is needed), and implement the two methods next to `matrix_outbox_cursor` (line 859):

```rust
    async fn matrix_gateway_cursor(
        &self,
        gateway_id: &str,
    ) -> Result<Option<MatrixGatewayCursorRecord>, CoreError> {
        Ok(self
            .matrix_gateway_cursors
            .lock()
            .expect("matrix gateway cursors lock")
            .get(gateway_id)
            .cloned())
    }

    async fn advance_matrix_gateway_cursor(
        &self,
        input: MatrixGatewayCursorInput,
    ) -> Result<MatrixGatewayCursorRecord, CoreError> {
        let gateway_id =
            normalize_required_text(&input.gateway_id, 256, "matrix gateway id required")?;
        let mut cursors = self
            .matrix_gateway_cursors
            .lock()
            .expect("matrix gateway cursors lock");
        let existing = cursors.get(&gateway_id).cloned();
        let record_version = match (existing.as_ref(), input.expected_version) {
            (None, None) => 1,
            (Some(current), Some(expected)) if current.record_version == expected => expected + 1,
            _ => {
                return Err(CoreError::Store(format!(
                    "conflict: matrix gateway cursor '{gateway_id}' record version mismatch"
                )));
            }
        };
        let now = 0;
        let record = MatrixGatewayCursorRecord {
            gateway_id: gateway_id.clone(),
            sync_token: input.sync_token,
            last_event_id: input.last_event_id,
            record_version,
            created_at: existing.as_ref().map_or(now, |current| current.created_at),
            updated_at: now,
        };
        cursors.insert(gateway_id, record.clone());
        Ok(record)
    }
```

Add `MatrixGatewayCursorInput, MatrixGatewayCursorRecord` to this file's `crate::host::{…}` import list.

- [ ] **Step 7: Run the HTTP test**

Run: `cargo nextest run -p agentd-bin --test daemon_http -E 'test(matrix_gateway_cursor)'`
Expected: PASS.
Run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS (the fake compiles against the widened trait).

- [ ] **Step 8: Write the failing bridge-client test**

Append to `crates/agentd-matrix/tests/http_backend.rs` (read the top of that file first and reuse its existing fake-agentd-server harness and `AgentdHttpBackend` construction verbatim — do not build a new one):

```rust
#[test]
fn http_backend_reads_and_advances_the_gateway_cursor_over_http() {
    let server = FakeAgentd::start(vec![
        // GET cursor
        r#"{"cursor":{"gatewayId":"matrix-bridge","syncToken":"s_batch_1","lastEventId":"$e1","recordVersion":3,"created_at":0,"updated_at":0}}"#
            .to_string(),
        // PUT cursor
        r#"{"ok":true,"cursor":{"gatewayId":"matrix-bridge","syncToken":"s_batch_2","lastEventId":"$e2","recordVersion":4,"created_at":0,"updated_at":0}}"#
            .to_string(),
    ]);
    let mut backend = AgentdHttpBackend::new(server.bridge_config());

    let cursor = backend.gateway_cursor().expect("gateway cursor");
    assert_eq!(cursor, Some((Some("s_batch_1".to_string()), 3)));

    let version = backend
        .advance_gateway_cursor(Some("s_batch_2"), Some("$e2"), Some(3))
        .expect("advance gateway cursor");
    assert_eq!(version, 4);

    let requests = server.requests();
    assert_eq!(
        requests[0].path,
        "/api/matrix/gateway/cursor?gatewayId=matrix-bridge"
    );
    assert_eq!(requests[1].method, "PUT");
    assert_eq!(requests[1].path, "/api/matrix/gateway/cursor");
}
```

If the harness's constructor or accessor names differ (`FakeAgentd::start`, `bridge_config()`, `requests()`), use the file's actual names — the assertions are what matter.

- [ ] **Step 9: Run it to verify it fails**

Run: `cargo nextest run -p agentd-matrix --test http_backend -E 'test(gateway_cursor)'`
Expected: FAIL — `no method named 'gateway_cursor' found for struct 'AgentdHttpBackend'`.

- [ ] **Step 10: Add the backend trait hooks and HTTP methods**

In `crates/agentd-matrix/src/lib.rs`, in `trait AgentdBridgeBackend` after `outbox_cursor` (line 3215):

```rust
    /// Read the daemon-owned inbound cursor: `(sync_token, record_version)`.
    ///
    /// The default is `Ok(None)` so fake backends in existing tests keep
    /// compiling; the HTTP backend overrides it.
    fn gateway_cursor(&mut self) -> Result<Option<(Option<String>, i64)>, BridgeError> {
        Ok(None)
    }

    /// Advance the daemon-owned inbound cursor, returning its new
    /// `record_version`.
    fn advance_gateway_cursor(
        &mut self,
        _sync_token: Option<&str>,
        _last_event_id: Option<&str>,
        _expected_version: Option<i64>,
    ) -> Result<i64, BridgeError> {
        Ok(0)
    }
```

In `impl AgentdHttpBackend`, next to `matrix_outbox_cursor` (line 4077):

```rust
    /// Read the daemon-owned Matrix gateway inbound cursor. A `404` means the
    /// cursor has never been written and is reported as `None`, not an error.
    pub fn read_gateway_cursor(&self) -> Result<Option<(Option<String>, i64)>, BridgeError> {
        let value = match self.request_json(
            "GET",
            "/api/matrix/gateway/cursor?gatewayId=matrix-bridge",
            None,
        ) {
            Ok(value) => value,
            Err(error) if error.is_not_found() => return Ok(None),
            Err(error) => return Err(error),
        };
        let cursor = value
            .get("cursor")
            .ok_or_else(|| BridgeError::backend("gateway cursor response missing cursor"))?;
        let record_version = cursor
            .get("recordVersion")
            .and_then(Value::as_i64)
            .ok_or_else(|| BridgeError::backend("gateway cursor response missing recordVersion"))?;
        let sync_token = cursor
            .get("syncToken")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        Ok(Some((sync_token, record_version)))
    }

    /// Advance the daemon-owned Matrix gateway inbound cursor under CAS.
    pub fn write_gateway_cursor(
        &self,
        sync_token: Option<&str>,
        last_event_id: Option<&str>,
        expected_version: Option<i64>,
    ) -> Result<i64, BridgeError> {
        let mut body = json!({ "gatewayId": "matrix-bridge" });
        if let Some(token) = sync_token {
            body["syncToken"] = json!(token);
        }
        if let Some(event_id) = last_event_id {
            body["lastEventId"] = json!(event_id);
        }
        if let Some(version) = expected_version {
            body["expectedVersion"] = json!(version);
        }
        let value = self.request_json("PUT", "/api/matrix/gateway/cursor", Some(body))?;
        value
            .get("cursor")
            .and_then(|cursor| cursor.get("recordVersion"))
            .and_then(Value::as_i64)
            .ok_or_else(|| BridgeError::backend("gateway cursor response missing recordVersion"))
    }
```

And in `impl AgentdBridgeBackend for AgentdHttpBackend` next to `outbox_cursor` (line 4340):

```rust
    fn gateway_cursor(&mut self) -> Result<Option<(Option<String>, i64)>, BridgeError> {
        self.read_gateway_cursor()
    }

    fn advance_gateway_cursor(
        &mut self,
        sync_token: Option<&str>,
        last_event_id: Option<&str>,
        expected_version: Option<i64>,
    ) -> Result<i64, BridgeError> {
        self.write_gateway_cursor(sync_token, last_event_id, expected_version)
    }
```

`BridgeError` has no `is_not_found()` today. Add it, backed by the HTTP status `request_json` already sees when it classifies a non-2xx response — locate that branch with `grep -n "fn request_json" -A 60 crates/agentd-matrix/src/lib.rs`, carry the status onto the backend error variant as a `not_found: bool`, and expose:

```rust
impl BridgeError {
    /// Whether this error came from an HTTP 404. A missing gateway cursor is a
    /// normal first-run state, not a bridge failure.
    #[must_use]
    pub const fn is_not_found(&self) -> bool {
        matches!(self, Self::Backend { not_found: true, .. })
    }
}
```

Match the real variant name and shape of `BridgeError`'s backend variant rather than the placeholder `Backend { not_found, .. }` written here.

- [ ] **Step 11: Carry the cursor through `BridgeState` and `run_once`**

Extend `BridgeState` (line 72). It currently derives `Copy`; adding a `String` field means dropping `Copy`, so change the derive list from `#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]` to `#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]`. `pub const fn state(&self) -> &BridgeState` (line 3264) already returns a reference and needs no change, but any call site that relied on `Copy` (e.g. `let state = *runtime.state();`) must become `.clone()` — fix each one the compiler flags.

```rust
pub struct BridgeState {
    next_from_seq: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    sync_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cursor_version: Option<i64>,
}
```

Add accessors next to `next_from_seq` (line 114):

```rust
    /// Last Matrix sync token confirmed by the daemon-owned cursor.
    #[must_use]
    pub fn sync_token(&self) -> Option<&str> {
        self.sync_token.as_deref()
    }

    /// `record_version` of the daemon-owned cursor this state last observed.
    #[must_use]
    pub const fn cursor_version(&self) -> Option<i64> {
        self.cursor_version
    }
```

`BridgeState::new` stays `pub const fn new(next_from_seq: i64) -> Self`; it now fills the two new fields with `None`, so drop `const`:

```rust
    /// Build bridge state from a previously confirmed outbox sequence.
    #[must_use]
    pub fn new(next_from_seq: i64) -> Self {
        Self {
            next_from_seq,
            sync_token: None,
            cursor_version: None,
        }
    }
```

In `BridgeRuntime::run_once` (line 3276), before the room-registration loop, seed from the daemon and after the inbound loop write back:

```rust
        // The daemon owns the inbound cursor. Seed from it every iteration so a
        // restarted gateway resumes where the daemon says it left off, not
        // where this process's local file happens to say.
        if let Some((sync_token, version)) = self.backend.gateway_cursor()? {
            self.state.sync_token = sync_token;
            self.state.cursor_version = Some(version);
        }

        for room in self.transport.room_registrations()? {
            self.backend.register_room(room)?;
            report.registered_rooms += 1;
        }

        let mut last_inbound_event_id = None;
        for event in self.transport.inbound_events()? {
            last_inbound_event_id = Some(event.event_id.clone());
            self.backend.post_inbound(event)?;
            report.inbound_forwarded += 1;
        }
        if let Some(event_id) = last_inbound_event_id {
            // Advance only after every event in this batch is durably accepted
            // by the daemon, so a crash mid-batch replays the batch rather than
            // skipping its tail.
            let version = self.backend.advance_gateway_cursor(
                self.state.sync_token.as_deref(),
                Some(event_id.as_str()),
                self.state.cursor_version,
            )?;
            self.state.cursor_version = Some(version);
        }
```

- [ ] **Step 12: Run the bridge tests**

Run: `cargo nextest run -p agentd-matrix --test http_backend`
Expected: PASS.
Run: `cargo nextest run -p agentd-matrix --test bridge_runtime`
Expected: PASS — the default `gateway_cursor`/`advance_gateway_cursor` hooks keep the fake backends compiling and inert.
Run: `cargo nextest run -p agentd-matrix --test client_bridge_once`
Expected: PASS.
Run: `cargo nextest run -p agentd-matrix --test file_transport`
Expected: PASS.

- [ ] **Step 13: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
cargo clippy --all-targets -p agentd-surface -- -D warnings
cargo clippy --all-targets -p agentd-matrix -- -D warnings
cargo clippy --all-targets -p agentd-bin -- -D warnings
git add crates/agentd-surface/src/host.rs crates/agentd-surface/src/http.rs \
        crates/agentd-surface/src/test_support.rs crates/agentd-bin/src/host.rs \
        crates/agentd-matrix/src/lib.rs crates/agentd-bin/tests/daemon_http.rs \
        crates/agentd-matrix/tests/http_backend.rs
git commit -m "feat(matrix): serve the gateway inbound cursor over HTTP to the remote bridge"
```

---

## Task 4: Migration 0029 — canonical `matrix_commands` with the room/project dedup constraint

**Files:**
- Create: `crates/agentd-store/migrations/0029_matrix_commands.sql`
- Modify: `crates/agentd-store/src/matrix_bridge_repo.rs` (append)
- Modify: `crates/agentd-store/tests/migration.rs` (`"28"` → `"29"` sweep + new table assertion)
- Modify: `crates/agentd-store/tests/operational_doctor.rs:23` (`28` → `29`)
- Test: `crates/agentd-store/tests/matrix_bridge.rs` (append)

**Interfaces:**
- Consumes: Task 2's `required`/`clean_opt` usage pattern in the same module.
- Produces:
  - `pub fn matrix_command_id(room_id: &str, event_id: &str) -> String` — canonical, deterministic: `mxc_` + the first 32 lowercase hex chars of `sha256("agentd.matrix.command.v1\x1f{room_id}\x1f{event_id}")`.
  - `pub fn matrix_command_dedup_key(body: &str) -> String` — the first 32 lowercase hex chars of `sha256` over the minimally normalized body.
  - `pub struct MatrixCommandRunPlan { pub label: String, pub owner: String, pub assignee: String, pub description: String }` (`Serialize + Deserialize`)
  - `pub struct MatrixCommandInput { pub event_id: String, pub room_id: String, pub project_id: Option<String>, pub sender_mxid: String, pub route: String, pub body: String, pub open: bool, pub run_request: Option<MatrixCommandRunPlan> }`
  - `pub struct MatrixCommandRecord { pub command_id: String, pub event_id: String, pub room_id: String, pub project_key: String, pub dedup_key: String, pub sender_mxid: String, pub route: String, pub status: String, pub message_id: Option<String>, pub run_id: Option<String>, pub run_request_json: Option<String>, pub record_version: i64, pub created_at: i64, pub updated_at: i64 }`
  - `pub async fn get_command(pool: &SqlitePool, command_id: &str) -> Result<Option<MatrixCommandRecord>, StoreError>`
  - `pub async fn list_accepted_commands(pool: &SqlitePool) -> Result<Vec<MatrixCommandRecord>, StoreError>` — `status = 'accepted' AND run_id IS NULL`, oldest first.

  Task 5 additionally adds a connection-scoped `insert_command_in_transaction`; Task 6 adds `bind_command_run`. Both live in this module and are specified in their own tasks.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/matrix_bridge.rs`:

```rust
#[tokio::test]
async fn matrix_command_id_is_canonical_and_deterministic() {
    let first = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-1");
    let again = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-1");
    let other_event = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-2");
    let other_room = matrix_bridge_repo::matrix_command_id("!other:matrix.test", "$event-1");

    assert_eq!(first, again, "the same event always yields the same command id");
    assert_ne!(first, other_event);
    assert_ne!(first, other_room);
    assert!(first.starts_with("mxc_"), "{first}");
    assert_eq!(first.len(), 4 + 32, "{first}");
}

#[tokio::test]
async fn matrix_command_dedup_key_ignores_case_and_surrounding_whitespace() {
    let plain = matrix_bridge_repo::matrix_command_dedup_key("Ship  the   patch");
    let noisy = matrix_bridge_repo::matrix_command_dedup_key("  ship the patch  ");
    let different = matrix_bridge_repo::matrix_command_dedup_key("ship the other patch");
    assert_eq!(plain, noisy);
    assert_ne!(plain, different);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-store --test matrix_bridge -E 'test(matrix_command)'`
Expected: FAIL — `cannot find function 'matrix_command_id' in module`.

- [ ] **Step 3: Write the migration**

Create `crates/agentd-store/migrations/0029_matrix_commands.sql`:

```sql
-- M4 Plan A: the canonical Matrix command record.
--
-- `command_id` is derived deterministically from (room_id, event_id), so a
-- replayed Matrix event recomputes the identical id without a read. The
-- partial unique index is the room/project dedup constraint: at most one OPEN
-- command per (room, project, payload). It is partial on purpose — a full
-- unique index would make an ordinary chat room reject the second "ok" anyone
-- types, because plain chat messages are recorded here as `settled` and must
-- never occupy the slot.
--
-- `project_key` is NOT NULL DEFAULT '' rather than a nullable `project_id`:
-- SQLite treats NULLs as distinct inside a unique index, so an unbound room
-- would otherwise escape the constraint entirely.
CREATE TABLE matrix_commands (
    command_id     TEXT PRIMARY KEY CHECK (length(trim(command_id)) > 0),
    event_id       TEXT NOT NULL UNIQUE CHECK (length(trim(event_id)) > 0),
    room_id        TEXT NOT NULL CHECK (length(trim(room_id)) > 0),
    project_key    TEXT NOT NULL DEFAULT '',
    dedup_key      TEXT NOT NULL CHECK (length(trim(dedup_key)) > 0),
    sender_mxid    TEXT NOT NULL CHECK (length(trim(sender_mxid)) > 0),
    route          TEXT NOT NULL CHECK (length(trim(route)) > 0),
    status         TEXT NOT NULL CHECK (status IN ('accepted', 'running', 'settled', 'rejected')),
    message_id     TEXT,
    run_id         TEXT,
    -- The run this command asks agentd to create, as JSON. NULL for plain
    -- chat. Task 6's sweep reads it; nothing else does.
    run_request_json TEXT,
    record_version INTEGER NOT NULL DEFAULT 1 CHECK (record_version > 0),
    created_at     INTEGER NOT NULL,
    updated_at     INTEGER NOT NULL
);

CREATE UNIQUE INDEX idx_matrix_commands_open_room_project
    ON matrix_commands(room_id, project_key, dedup_key)
    WHERE status IN ('accepted', 'running');

CREATE INDEX idx_matrix_commands_room_created
    ON matrix_commands(room_id, created_at);

CREATE INDEX idx_matrix_commands_status_created
    ON matrix_commands(status, created_at);

UPDATE schema_meta SET value = '29' WHERE key = 'version';
```

- [ ] **Step 4: Write the id derivation and the read helpers**

Append to `crates/agentd-store/src/matrix_bridge_repo.rs`. Add `use sha2::{Digest, Sha256};` to the imports at the top.

```rust
/// The canonical agentd command id for one Matrix event.
///
/// Deterministic: a replayed event recomputes the identical id with no read,
/// which is what lets the inbound transaction be a pure insert-or-conflict.
/// The domain prefix and the unit-separator framing keep `(room, event)` pairs
/// from colliding across concatenations.
#[must_use]
pub fn matrix_command_id(room_id: &str, event_id: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(b"agentd.matrix.command.v1\x1f");
    hasher.update(room_id.trim().as_bytes());
    hasher.update(b"\x1f");
    hasher.update(event_id.trim().as_bytes());
    format!("mxc_{}", hex32(&hasher.finalize()))
}

/// The room/project dedup key for one command payload.
///
/// The normalization here is deliberately minimal — trim, ASCII-lowercase,
/// collapse internal whitespace. Full command normalization (mention
/// stripping, bang-command parsing) is M4 Plan B, and it replaces this
/// function and the dedup key together.
#[must_use]
pub fn matrix_command_dedup_key(body: &str) -> String {
    let normalized = body
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    let mut hasher = Sha256::new();
    hasher.update(b"agentd.matrix.dedup.v1\x1f");
    hasher.update(normalized.as_bytes());
    hex32(&hasher.finalize())
}

fn hex32(digest: &[u8]) -> String {
    digest
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The run a Matrix command asks agentd to create, as stored on the command.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MatrixCommandRunPlan {
    pub label: String,
    pub owner: String,
    pub assignee: String,
    pub description: String,
}

/// One inbound Matrix command as accepted by the gateway.
#[derive(Debug, Clone)]
pub struct MatrixCommandInput {
    pub event_id: String,
    pub room_id: String,
    pub project_id: Option<String>,
    pub sender_mxid: String,
    pub route: String,
    pub body: String,
    /// `true` when the command requests a run and must hold the open-dedup
    /// slot until it settles; `false` for plain chat, which is recorded
    /// `settled` and never contends. Always equals `run_request.is_some()` for
    /// callers going through the inbound route, but stays a separate field so
    /// the index predicate is explicit at the insert site.
    pub open: bool,
    /// The run to create, or `None` for plain chat.
    pub run_request: Option<MatrixCommandRunPlan>,
}

/// Durable canonical command record.
#[derive(Debug, Clone)]
pub struct MatrixCommandRecord {
    pub command_id: String,
    pub event_id: String,
    pub room_id: String,
    pub project_key: String,
    pub dedup_key: String,
    pub sender_mxid: String,
    pub route: String,
    pub status: String,
    pub message_id: Option<String>,
    pub run_id: Option<String>,
    pub run_request_json: Option<String>,
    pub record_version: i64,
    pub created_at: i64,
    pub updated_at: i64,
}

const COMMAND_SELECT_SQL: &str = "SELECT command_id, event_id, room_id, project_key, dedup_key, \
     sender_mxid, route, status, message_id, run_id, run_request_json, record_version, \
     created_at, updated_at FROM matrix_commands";

pub async fn get_command(
    pool: &SqlitePool,
    command_id: &str,
) -> Result<Option<MatrixCommandRecord>, StoreError> {
    let command_id = required(command_id.to_string(), "matrix command id required")?;
    let row = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(command_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|row| row_to_command(&row)))
}

/// Commands that were accepted but have no run yet, oldest first.
pub async fn list_accepted_commands(
    pool: &SqlitePool,
) -> Result<Vec<MatrixCommandRecord>, StoreError> {
    let rows = sqlx::query(&format!(
        "{COMMAND_SELECT_SQL} WHERE status = 'accepted' AND run_id IS NULL \
         ORDER BY created_at ASC, command_id ASC"
    ))
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_command).collect())
}

fn row_to_command(row: &sqlx::sqlite::SqliteRow) -> MatrixCommandRecord {
    MatrixCommandRecord {
        command_id: row.get("command_id"),
        event_id: row.get("event_id"),
        room_id: row.get("room_id"),
        project_key: row.get("project_key"),
        dedup_key: row.get("dedup_key"),
        sender_mxid: row.get("sender_mxid"),
        route: row.get("route"),
        status: row.get("status"),
        message_id: row.get("message_id"),
        run_id: row.get("run_id"),
        run_request_json: row.get("run_request_json"),
        record_version: row.get("record_version"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}
```

- [ ] **Step 5: Sweep the schema-version assertions and assert the table**

Run: `sed -i '' 's/assert_eq!(version, "28")/assert_eq!(version, "29")/g' crates/agentd-store/tests/migration.rs`
Change `crates/agentd-store/tests/operational_doctor.rs:23` to `assert_eq!(report.schema_version, 29);`.
Verify: `grep -rn '"28"\|schema_version, 28' crates/agentd-store/tests/` returns no hits.

Append to `crates/agentd-store/tests/migration.rs`:

```rust
#[tokio::test]
async fn migration_creates_matrix_command_table_with_the_open_dedup_index() {
    let (store, _dir) = open_temp().await;
    let table: Option<String> = sqlx::query_scalar(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = 'matrix_commands'",
    )
    .fetch_optional(store.pool())
    .await
    .expect("query sqlite_master");
    assert_eq!(table.as_deref(), Some("matrix_commands"));

    let index: Option<String> = sqlx::query_scalar(
        "SELECT sql FROM sqlite_master WHERE type = 'index' \
         AND name = 'idx_matrix_commands_open_room_project'",
    )
    .fetch_optional(store.pool())
    .await
    .expect("query sqlite_master index");
    let index = index.expect("open dedup index exists");
    assert!(index.contains("UNIQUE"), "{index}");
    assert!(index.contains("room_id"), "{index}");
    assert!(index.contains("project_key"), "{index}");
    assert!(index.contains("dedup_key"), "{index}");
    assert!(index.contains("accepted"), "{index}");

    let version: String = sqlx::query_scalar("SELECT value FROM schema_meta WHERE key = 'version'")
        .fetch_one(store.pool())
        .await
        .expect("schema version");
    assert_eq!(version, "29");
}
```

- [ ] **Step 6: Run the tests**

Run: `cargo nextest run -p agentd-store --test matrix_bridge`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test migration`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test operational_doctor`
Expected: PASS.

- [ ] **Step 7: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
git add crates/agentd-store/migrations/0029_matrix_commands.sql \
        crates/agentd-store/src/matrix_bridge_repo.rs \
        crates/agentd-store/tests/matrix_bridge.rs \
        crates/agentd-store/tests/migration.rs \
        crates/agentd-store/tests/operational_doctor.rs
git commit -m "feat(matrix): add canonical matrix commands with a room/project dedup constraint"
```

---

## Task 5: One `BEGIN IMMEDIATE` for event, command, inbox message, and outbox

**Files:**
- Modify: `crates/agentd-store/src/message_repo.rs` (extract connection-scoped `_on` variants)
- Modify: `crates/agentd-store/src/relay_repo.rs` (extract `append_relay_stream_event_on`)
- Modify: `crates/agentd-store/src/matrix_bridge_repo.rs` (`record_event_on`, `insert_command_on`, `accept_inbound_event`)
- Modify: `crates/agentd-surface/src/host.rs` (`MatrixInboundMessageInput` gains `run_request`; `MatrixInboundMessageResult` gains `command_id`)
- Modify: `crates/agentd-surface/src/test_support.rs` (fake returns a `command_id`)
- Modify: `crates/agentd-bin/src/host.rs:1957-2114` (`post_matrix_inbound_message` rewrite)
- Test: `crates/agentd-store/tests/matrix_bridge.rs`, `crates/agentd-bin/tests/daemon_http.rs`

**Interfaces:**
- Consumes: Task 4's `matrix_command_id`, `matrix_command_dedup_key`, `MatrixCommandInput`, `MatrixCommandRecord`, `get_command`.
- Produces:
  - `message_repo::insert_direct_message_on(conn: &mut SqliteConnection, input: DirectMessageInput) -> Result<DirectMessageRecord, StoreError>` and `message_repo::insert_group_message_on(conn: &mut SqliteConnection, input: GroupMessageInput) -> Result<GroupMessageRecord, StoreError>`; the existing pool-taking functions keep their signatures and delegate.
  - `relay_repo::append_relay_stream_event_on(conn: &mut SqliteConnection, event: &str, payload: Value) -> Result<RelayStreamEventRecord, StoreError>`; the pool version delegates.
  - `matrix_bridge_repo::accept_inbound_event(pool, input: MatrixInboundAcceptance) -> Result<MatrixInboundAcceptanceResult, StoreError>`, where

    ```rust
    pub struct MatrixInboundAcceptance {
        pub command: MatrixCommandInput,
        pub direct: Option<crate::message_repo::DirectMessageInput>,
        pub group: Option<crate::message_repo::GroupMessageInput>,
        pub relay_payload: serde_json::Value,
    }

    pub struct MatrixInboundAcceptanceResult {
        pub command: MatrixCommandRecord,
        pub duplicate: bool,
        pub direct: Option<crate::message_repo::DirectMessageRecord>,
        pub group: Option<crate::message_repo::GroupMessageRecord>,
    }
    ```
  - Surface `MatrixInboundMessageInput.run_request: Option<MatrixCommandRunRequest>` where `pub struct MatrixCommandRunRequest { pub label: String, pub owner: String, pub assignee: String, pub description: String }` (wire: `label`, `owner`, `assignee`, `description`), and `MatrixInboundMessageResult.command_id: Option<String>` (wire `commandId`). The host maps `MatrixCommandRunRequest` (surface) onto `MatrixCommandRunPlan` (store) field for field; Task 6 reads it back off the command row as `run_request_json`, never through the surface type.

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/matrix_bridge.rs`:

```rust
#[tokio::test]
async fn accept_inbound_event_writes_event_command_message_and_outbox_atomically() {
    let (store, _dir) = open_temp().await;
    matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text("!dm:matrix.test"),
            project_id: None,
            group_name: None,
            agent_name: Some(text("codex-worker")),
            trusted: true,
            trust_reason: text("managed"),
            inviter_mxid: None,
        },
    )
    .await
    .expect("upsert room");

    let command_id = matrix_bridge_repo::matrix_command_id("!dm:matrix.test", "$dm-1");
    let acceptance = || matrix_bridge_repo::MatrixInboundAcceptance {
        command: matrix_bridge_repo::MatrixCommandInput {
            event_id: text("$dm-1"),
            room_id: text("!dm:matrix.test"),
            project_id: None,
            sender_mxid: text("@alice:matrix.test"),
            route: text("agent"),
            body: text("please review the patch"),
            open: false,
            run_request: None,
        },
        direct: Some(agentd_store::message_repo::DirectMessageInput {
            message_id: Some(format!("msg_{command_id}")),
            ts: None,
            from: text("alice"),
            to: text("codex-worker"),
            message_type: Some(text("human")),
            priority: None,
            summary: text("please review the patch"),
            full: text("please review the patch"),
            reply_to: None,
            source: Some(text("matrix")),
            source_room: Some(text("!dm:matrix.test")),
            sender_mxid: Some(text("@alice:matrix.test")),
            trust_level: Some(text("external")),
            from_id: None,
            schema: None,
            attachments: Vec::new(),
        }),
        group: None,
        relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
    };

    let first = matrix_bridge_repo::accept_inbound_event(store.pool(), acceptance())
        .await
        .expect("first acceptance");
    assert!(!first.duplicate);
    assert_eq!(first.command.command_id, command_id);
    assert_eq!(first.command.status, "settled");
    let message_id = first.direct.expect("direct message").id;

    // Replay: the same event id yields the same command and creates nothing.
    let second = matrix_bridge_repo::accept_inbound_event(store.pool(), acceptance())
        .await
        .expect("replayed acceptance");
    assert!(second.duplicate);
    assert_eq!(second.command.command_id, command_id);

    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM direct_messages")
        .fetch_one(store.pool())
        .await
        .expect("count messages");
    assert_eq!(messages, 1, "replay must not create a second message");
    let outbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM relay_stream_events")
        .fetch_one(store.pool())
        .await
        .expect("count outbox");
    assert_eq!(outbox, 1, "replay must not enqueue a second outbox event");
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matrix_bridge_events")
        .fetch_one(store.pool())
        .await
        .expect("count events");
    assert_eq!(events, 1);
    let commands: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matrix_commands")
        .fetch_one(store.pool())
        .await
        .expect("count commands");
    assert_eq!(commands, 1);

    let message_from_id: Option<String> =
        sqlx::query_scalar("SELECT message_id FROM matrix_commands WHERE command_id = ?")
            .bind(&command_id)
            .fetch_one(store.pool())
            .await
            .expect("command message id");
    assert_eq!(message_from_id.as_deref(), Some(message_id.as_str()));
}

#[tokio::test]
async fn accept_inbound_event_rejects_a_second_open_command_for_one_room_and_project() {
    let (store, _dir) = open_temp().await;
    matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text("!ops:matrix.test"),
            project_id: None,
            group_name: None,
            agent_name: Some(text("codex-worker")),
            trusted: true,
            trust_reason: text("managed"),
            inviter_mxid: None,
        },
    )
    .await
    .expect("upsert room");

    let open_command = |event_id: &str| matrix_bridge_repo::MatrixInboundAcceptance {
        command: matrix_bridge_repo::MatrixCommandInput {
            event_id: text(event_id),
            room_id: text("!ops:matrix.test"),
            project_id: Some(text("project-ops")),
            sender_mxid: text("@alice:matrix.test"),
            route: text("agent"),
            body: text("run the build"),
            open: true,
            run_request: Some(matrix_bridge_repo::MatrixCommandRunPlan {
                label: text("build"),
                owner: text("alice"),
                assignee: text("codex-worker"),
                description: text("run the build"),
            }),
        },
        direct: None,
        group: None,
        relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
    };

    let first = matrix_bridge_repo::accept_inbound_event(store.pool(), open_command("$run-1"))
        .await
        .expect("first open command");
    assert_eq!(first.command.status, "accepted");
    assert_eq!(first.command.project_key, "project-ops");

    // A *different* Matrix event carrying the same payload in the same room and
    // project is a duplicate execution request, not a second execution.
    let clash = matrix_bridge_repo::accept_inbound_event(store.pool(), open_command("$run-2"))
        .await
        .expect_err("second open command must conflict");
    let message = clash.to_string();
    assert!(message.contains("already open"), "{message}");
    assert!(!message.ends_with("changed concurrently"), "{message}");

    // A plain (non-open) command with the same payload does not contend.
    let mut chat = open_command("$chat-1");
    chat.command.open = false;
    chat.command.run_request = None;
    matrix_bridge_repo::accept_inbound_event(store.pool(), chat)
        .await
        .expect("plain chat is never blocked by the open-dedup slot");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-store --test matrix_bridge -E 'test(accept_inbound_event)'`
Expected: FAIL — `cannot find function 'accept_inbound_event' in module`.

- [ ] **Step 3: Extract the connection-scoped message inserts**

In `crates/agentd-store/src/message_repo.rs`, replace `insert_direct_message` (lines 283-338) in full with a pool wrapper plus a connection-scoped body. The body is the existing one, with `.execute(pool)` changed to `.execute(&mut *connection)` and the trailing lookup changed to `get_direct_message_on`:

```rust
pub async fn insert_direct_message(
    pool: &SqlitePool,
    input: DirectMessageInput,
) -> Result<DirectMessageRecord, StoreError> {
    let mut connection = pool.acquire().await?;
    insert_direct_message_on(&mut connection, input).await
}

/// Insert one direct message on the caller's connection, so it can join a
/// wider `BEGIN IMMEDIATE` (the Matrix inbound handoff needs the message, its
/// command row, and its outbox event to land together or not at all).
pub async fn insert_direct_message_on(
    connection: &mut sqlx::SqliteConnection,
    input: DirectMessageInput,
) -> Result<DirectMessageRecord, StoreError> {
    let id = clean_opt(input.message_id).unwrap_or_else(generate_message_id);
    let from = required(input.from, "message from required")?;
    let to = required(input.to, "message to required")?;
    let message_type = clean_opt(input.message_type).unwrap_or_else(|| "human".to_string());
    let priority = clean_opt(input.priority).unwrap_or_else(|| "normal".to_string());
    let summary = required(input.summary, "message summary required")?;
    let full = input.full;
    let reply_to = clean_opt(input.reply_to);
    let source = clean_opt(input.source).unwrap_or_else(|| "api".to_string());
    let source_room = clean_opt(input.source_room);
    let sender_mxid = clean_opt(input.sender_mxid);
    let trust_level = clean_opt(input.trust_level);
    let from_id = clean_opt(input.from_id);
    let schema_json = input
        .schema
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let attachments_json = serde_json::to_string(&input.attachments)?;
    let ts = input.ts.unwrap_or_else(now_unix_ms);
    let created_at = now_unix();

    sqlx::query(
        "INSERT INTO direct_messages \
         (id, ts, from_agent, to_agent, message_type, priority, summary, full, \
          reply_to, source, source_room, sender_mxid, trust_level, from_id, \
          schema_json, attachments_json, created_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         ON CONFLICT(id) DO NOTHING",
    )
    .bind(&id)
    .bind(ts)
    .bind(&from)
    .bind(&to)
    .bind(&message_type)
    .bind(&priority)
    .bind(&summary)
    .bind(&full)
    .bind(reply_to.as_deref())
    .bind(&source)
    .bind(source_room.as_deref())
    .bind(sender_mxid.as_deref())
    .bind(trust_level.as_deref())
    .bind(from_id.as_deref())
    .bind(schema_json.as_deref())
    .bind(&attachments_json)
    .bind(created_at)
    .execute(&mut *connection)
    .await?;

    get_direct_message_on(&mut *connection, &id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("direct message '{id}' is missing")))
}
```

Do the same mechanical split for `insert_group_message` (line 373 onward): keep its whole existing body, change its single `.execute(pool)` to `.execute(&mut *connection)`, change its trailing `get_group_message(pool, &id)` to `get_group_message_on(&mut *connection, &id)`, and add the pool wrapper:

```rust
pub async fn insert_group_message(
    pool: &SqlitePool,
    input: GroupMessageInput,
) -> Result<GroupMessageRecord, StoreError> {
    let mut connection = pool.acquire().await?;
    insert_group_message_on(&mut connection, input).await
}
```

Then change `get_direct_message`/`get_group_message` (lines 532-552) into `_on` variants with pool wrappers:

```rust
async fn get_direct_message(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<DirectMessageRecord>, StoreError> {
    let mut connection = pool.acquire().await?;
    get_direct_message_on(&mut connection, id).await
}

async fn get_direct_message_on(
    connection: &mut sqlx::SqliteConnection,
    id: &str,
) -> Result<Option<DirectMessageRecord>, StoreError> {
    let row = sqlx::query(direct_message_select_sql("WHERE id = ?").as_str())
        .bind(id)
        .fetch_optional(&mut *connection)
        .await?;
    row.map(|r| row_to_message(&r)).transpose()
}
```

and the analogous `get_group_message_on`. `insert_group_message` also reads `group_members` — check with `grep -n "group_members(pool" crates/agentd-store/src/message_repo.rs`; if its record construction calls `group_members(pool, …)`, give that a `group_members_on(connection, …)` variant the same way.

- [ ] **Step 4: Extract the connection-scoped relay append**

In `crates/agentd-store/src/relay_repo.rs`, replace `append_relay_stream_event` (line 203) with:

```rust
pub async fn append_relay_stream_event(
    pool: &SqlitePool,
    event: &str,
    payload: Value,
) -> Result<RelayStreamEventRecord, StoreError> {
    let mut connection = pool.acquire().await?;
    append_relay_stream_event_on(&mut connection, event, payload).await
}

/// Append one relay-stream (Matrix outbox) event on the caller's connection.
pub async fn append_relay_stream_event_on(
    connection: &mut sqlx::SqliteConnection,
    event: &str,
    payload: Value,
) -> Result<RelayStreamEventRecord, StoreError> {
    let event = required(event.to_string(), "stream event required")?;
    let payload = match payload {
        Value::Object(_) => payload,
        other => json!({ "value": other }),
    };
    let payload_json = serde_json::to_string(&payload)?;
    let created_at = now_unix();
    let result = sqlx::query(
        "INSERT INTO relay_stream_events (event, payload_json, created_at) VALUES (?, ?, ?)",
    )
    .bind(&event)
    .bind(payload_json)
    .bind(created_at)
    .execute(&mut *connection)
    .await?;
    let seq = result.last_insert_rowid();
    Ok(RelayStreamEventRecord {
        seq,
        event,
        payload,
        created_at,
    })
}
```

- [ ] **Step 5: Write the transactional acceptance**

Append to `crates/agentd-store/src/matrix_bridge_repo.rs`:

```rust
/// Everything one accepted inbound Matrix event writes, as one unit.
#[derive(Debug, Clone)]
pub struct MatrixInboundAcceptance {
    pub command: MatrixCommandInput,
    pub direct: Option<crate::message_repo::DirectMessageInput>,
    pub group: Option<crate::message_repo::GroupMessageInput>,
    pub relay_payload: serde_json::Value,
}

/// What the acceptance produced. `duplicate` means the event was already
/// accepted and nothing new was written.
#[derive(Debug, Clone)]
pub struct MatrixInboundAcceptanceResult {
    pub command: MatrixCommandRecord,
    pub duplicate: bool,
    pub direct: Option<crate::message_repo::DirectMessageRecord>,
    pub group: Option<crate::message_repo::GroupMessageRecord>,
}

/// Accept one inbound Matrix event: processed-event row, canonical command
/// row, inbox message, and outbox event, in one `BEGIN IMMEDIATE` or none.
///
/// Before this, the four writes were independent pool calls behind a
/// read-only duplicate check, so a crash between the message insert and the
/// event insert let the replay create a second message under a fresh ULID.
/// Here the message id is caller-supplied and derived from the command id, so
/// even a torn write is repaired by `ON CONFLICT(id) DO NOTHING`.
///
/// # Errors
/// [`StoreError::Invariant`] on blank required fields;
/// [`StoreError::Conflict`] when an open command for the same
/// `(room, project, payload)` already exists.
pub async fn accept_inbound_event(
    pool: &SqlitePool,
    acceptance: MatrixInboundAcceptance,
) -> Result<MatrixInboundAcceptanceResult, StoreError> {
    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = accept_inbound_event_in_transaction(&mut connection, acceptance).await;
    match result {
        Ok(value) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(value)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn accept_inbound_event_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    acceptance: MatrixInboundAcceptance,
) -> Result<MatrixInboundAcceptanceResult, StoreError> {
    let MatrixInboundAcceptance {
        command,
        direct,
        group,
        relay_payload,
    } = acceptance;
    let event_id = required(command.event_id, "matrix event id required")?;
    let room_id = required(command.room_id, "matrix room id required")?;
    let sender_mxid = required(command.sender_mxid, "matrix sender mxid required")?;
    let route = required(command.route, "matrix route required")?;
    let project_key = clean_opt(command.project_id).unwrap_or_default();
    let dedup_key = matrix_command_dedup_key(&command.body);
    let command_id = matrix_command_id(&room_id, &event_id);
    let status = if command.open { "accepted" } else { "settled" };
    let run_request_json = command
        .run_request
        .as_ref()
        .map(serde_json::to_string)
        .transpose()?;
    let now = now_unix();

    // The duplicate check is inside the transaction, so a concurrent POST of
    // the same event cannot slip between the read and the writes.
    if let Some(row) = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(&command_id)
        .fetch_optional(&mut *connection)
        .await?
    {
        return Ok(MatrixInboundAcceptanceResult {
            command: row_to_command(&row),
            duplicate: true,
            direct: None,
            group: None,
        });
    }

    let inserted_event = sqlx::query(
        "INSERT INTO matrix_bridge_events \
         (event_id, room_id, sender_mxid, message_id, route, ignored, created_at) \
         VALUES (?, ?, ?, NULL, ?, 0, ?)",
    )
    .bind(&event_id)
    .bind(&room_id)
    .bind(&sender_mxid)
    .bind(&route)
    .bind(now)
    .execute(&mut *connection)
    .await?;
    if inserted_event.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix event '{event_id}' was already accepted"
        )));
    }

    let inserted_command = sqlx::query(
        "INSERT INTO matrix_commands \
         (command_id, event_id, room_id, project_key, dedup_key, sender_mxid, route, status, \
          message_id, run_id, run_request_json, record_version, created_at, updated_at) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, NULL, NULL, ?, 1, ?, ?)",
    )
    .bind(&command_id)
    .bind(&event_id)
    .bind(&room_id)
    .bind(&project_key)
    .bind(&dedup_key)
    .bind(&sender_mxid)
    .bind(&route)
    .bind(status)
    .bind(run_request_json.as_deref())
    .bind(now)
    .bind(now)
    .execute(&mut *connection)
    .await
    .map_err(|error| {
        if is_open_command_clash(&error) {
            // The partial unique index fired: an equivalent command for this
            // room and project is still running. Not "changed concurrently" —
            // the caller must not retry this, it must wait for the open one.
            StoreError::Conflict(format!(
                "matrix command for room '{room_id}' is already open"
            ))
        } else {
            StoreError::Sqlx(error)
        }
    })?;
    if inserted_command.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{command_id}' was already accepted"
        )));
    }

    let direct_record = match direct {
        Some(input) => {
            Some(crate::message_repo::insert_direct_message_on(&mut *connection, input).await?)
        }
        None => None,
    };
    let group_record = match group {
        Some(input) => {
            Some(crate::message_repo::insert_group_message_on(&mut *connection, input).await?)
        }
        None => None,
    };

    let message_id = direct_record
        .as_ref()
        .map(|record| record.id.clone())
        .or_else(|| group_record.as_ref().map(|record| record.id.clone()));
    if let Some(message_id) = message_id.as_deref() {
        let linked = sqlx::query(
            "UPDATE matrix_bridge_events SET message_id = ? WHERE event_id = ? AND message_id IS NULL",
        )
        .bind(message_id)
        .bind(&event_id)
        .execute(&mut *connection)
        .await?;
        if linked.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "matrix event '{event_id}' record version mismatch"
            )));
        }
        let linked_command = sqlx::query(
            "UPDATE matrix_commands SET message_id = ?, record_version = record_version + 1, \
             updated_at = ? WHERE command_id = ? AND record_version = 1",
        )
        .bind(message_id)
        .bind(now)
        .bind(&command_id)
        .execute(&mut *connection)
        .await?;
        if linked_command.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "matrix command '{command_id}' record version mismatch"
            )));
        }
    }

    let mut payload = match relay_payload {
        serde_json::Value::Object(_) => relay_payload,
        other => serde_json::json!({ "value": other }),
    };
    payload["commandId"] = serde_json::json!(command_id);
    if let Some(message_id) = message_id.as_deref() {
        payload["messageId"] = serde_json::json!(message_id);
    }
    crate::relay_repo::append_relay_stream_event_on(&mut *connection, "message", payload).await?;

    let row = sqlx::query(&format!("{COMMAND_SELECT_SQL} WHERE command_id = ?"))
        .bind(&command_id)
        .fetch_optional(&mut *connection)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("matrix command '{command_id}' is missing")))?;
    Ok(MatrixInboundAcceptanceResult {
        command: row_to_command(&row),
        duplicate: false,
        direct: direct_record,
        group: group_record,
    })
}

fn is_open_command_clash(error: &sqlx::Error) -> bool {
    error
        .as_database_error()
        .is_some_and(|db| db.message().contains("idx_matrix_commands_open_room_project"))
}
```

If `StoreError` has no `Sqlx` tuple variant with that exact name, check `crates/agentd-store/src/error.rs` and use the real variant (or `error.into()`).

- [ ] **Step 6: Run the store test**

Run: `cargo nextest run -p agentd-store --test matrix_bridge`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test messages`
Expected: PASS — the `_on` extraction is behaviour-preserving.
Run: `cargo nextest run -p agentd-store --test remote_relay`
Expected: PASS.

- [ ] **Step 7: Widen the surface types**

In `crates/agentd-surface/src/host.rs`, add to `MatrixInboundMessageInput` (after `trust_level`, line 352):

```rust
    /// Optional run request. M4 Plan A accepts and honours this field; M4 Plan
    /// B is what populates it from a normalized bang command. Absent — every
    /// call the bridge makes today — the command is recorded `settled` and
    /// behaviour is unchanged.
    #[serde(default, rename = "runRequest", alias = "run_request")]
    pub run_request: Option<MatrixCommandRunRequest>,
```

and next to it:

```rust
/// A run the inbound Matrix command asks agentd to create.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatrixCommandRunRequest {
    pub label: String,
    pub owner: String,
    pub assignee: String,
    pub description: String,
}
```

Add to `MatrixInboundMessageResult` (line 365):

```rust
    /// Canonical agentd command id for this Matrix event.
    #[serde(rename = "commandId")]
    pub command_id: Option<String>,
```

- [ ] **Step 8: Rewrite `post_matrix_inbound_message`**

Replace `crates/agentd-bin/src/host.rs:1957-2114` with the transactional version. The room/cutover/fence/`[AGENTIGNORE]` preamble at lines 1961-2040 is unchanged — keep it verbatim, and only add `command_id: None` to the two early-return `SurfaceMatrixInboundMessageResult` literals (the duplicate branch and the ignored branch). Replace everything from the `let from = …` binding (line 2042) to the end of the method with:

```rust
        let from =
            clean_optional_string(input.from).unwrap_or_else(|| matrix_sender_name(&sender_mxid));
        let trust_level =
            clean_optional_string(input.trust_level).or_else(|| Some("external".to_string()));
        let command_id = matrix_bridge_repo::matrix_command_id(&room_id, &event_id);
        // Deterministic: a replayed event reuses this exact id, so the message
        // insert's `ON CONFLICT(id) DO NOTHING` makes a torn write self-heal
        // instead of producing a second inbox message.
        let message_id = format!("msg_{command_id}");
        let run_request = input.run_request.as_ref().map(|request| {
            matrix_bridge_repo::MatrixCommandRunPlan {
                label: request.label.clone(),
                owner: request.owner.clone(),
                assignee: request.assignee.clone(),
                description: request.description.clone(),
            }
        });
        let open = run_request.is_some();

        let (route, direct, group) = if let Some(group_name) = room.group_name.clone() {
            (
                "group".to_string(),
                None,
                Some(message_repo::GroupMessageInput {
                    message_id: Some(message_id.clone()),
                    ts: None,
                    from,
                    group: group_name,
                    message_type: Some("human".to_string()),
                    priority: None,
                    summary: input.body.clone(),
                    full: input.body.clone(),
                    mentions: clean_string_vec(input.mentions),
                    reply_to: input.reply_to.clone(),
                    source: Some("matrix".to_string()),
                    schema: None,
                    attachments: Vec::new(),
                }),
            )
        } else if let Some(agent) = room.agent_name.clone() {
            (
                "agent".to_string(),
                Some(message_repo::DirectMessageInput {
                    message_id: Some(message_id.clone()),
                    ts: None,
                    from,
                    to: agent,
                    message_type: Some("human".to_string()),
                    priority: None,
                    summary: input.body.clone(),
                    full: input.body.clone(),
                    reply_to: input.reply_to.clone(),
                    source: Some("matrix".to_string()),
                    source_room: Some(room_id.clone()),
                    sender_mxid: Some(sender_mxid.clone()),
                    trust_level,
                    from_id: None,
                    schema: None,
                    attachments: Vec::new(),
                }),
                None,
            )
        } else {
            return Err(CoreError::Invariant("matrix room not trusted".to_string()));
        };

        let relay_payload = serde_json::json!({
            "kind": if route == "group" { "group" } else { "direct" },
            "source": "matrix",
            "roomId": room_id.clone(),
        });

        let accepted = matrix_bridge_repo::accept_inbound_event(
            self.store.pool(),
            matrix_bridge_repo::MatrixInboundAcceptance {
                command: matrix_bridge_repo::MatrixCommandInput {
                    event_id: event_id.clone(),
                    room_id: room_id.clone(),
                    project_id: room.project_id.clone(),
                    sender_mxid: sender_mxid.clone(),
                    route: route.clone(),
                    body: input.body.clone(),
                    open,
                    run_request,
                },
                direct,
                group,
                relay_payload,
            },
        )
        .await
        .map_err(core_from_store_error)?;

        let message = accepted
            .direct
            .map(surface_inbox_message)
            .or_else(|| accepted.group.map(surface_group_inbox_message));
        Ok(SurfaceMatrixInboundMessageResult {
            ok: true,
            duplicate: accepted.duplicate,
            ignored: false,
            route: accepted.command.route.clone(),
            event_id: accepted.command.event_id.clone(),
            message_id: accepted.command.message_id.clone(),
            command_id: Some(accepted.command.command_id),
            message,
        })
    }
```

Note two behaviour changes this makes deliberately: the duplicate branch now returns the stored `route`/`message_id` from the command row rather than the event row (they agree), and the second redundant `get_room` at old line 2006 is gone — the room read at line 1961 is reused. Keep the `!room.trusted` guard; move it up next to that first read.

Also change `post_matrix_inbound` in `crates/agentd-surface/src/http.rs:601-618` to classify conflicts:

```rust
        Err(CoreError::Invariant(message)) if message == "matrix room not trusted" => (
            StatusCode::FORBIDDEN,
            Json(json!({ "error": "matrix room not trusted" })),
        )
            .into_response(),
        // A second *open* command for one room and project is a 409, not a 500.
        Err(error) => task_error_response(error),
```

And update the fake in `crates/agentd-surface/src/test_support.rs` so every `MatrixInboundMessageResult` it constructs sets `command_id` — for the fake, `Some(format!("mxc_fake_{event_id}"))` is sufficient and the existing stored-result replay keeps it stable.

- [ ] **Step 9: Write the daemon-level replay test**

Append to `crates/agentd-bin/tests/daemon_http.rs`:

```rust
#[tokio::test]
async fn daemon_router_matrix_inbound_replay_never_creates_a_second_message() {
    let (app, _dir) = empty_router().await;
    let (agent_status, agent_body) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({ "name": "codex-worker", "runtime": "codex" }),
    )
    .await;
    assert_eq!(agent_status, StatusCode::OK, "body: {agent_body}");

    let (room_status, room_body) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!dm:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK, "body: {room_body}");

    let inbound = serde_json::json!({
        "eventId": "$dm-replay",
        "roomId": "!dm:matrix.test",
        "senderMxid": "@alice:matrix.test",
        "body": "please review the patch"
    });

    let (first_status, first_body) =
        post(app.clone(), "/api/matrix/inbound", inbound.clone()).await;
    assert_eq!(first_status, StatusCode::CREATED, "body: {first_body}");
    let first: serde_json::Value = serde_json::from_str(&first_body).expect("first json");
    let command_id = first["commandId"].as_str().expect("command id").to_string();
    assert!(command_id.starts_with("mxc_"), "{command_id}");
    let message_id = first["message"]["id"].as_str().expect("message id").to_string();

    for _ in 0..3 {
        let (status, body) = post(app.clone(), "/api/matrix/inbound", inbound.clone()).await;
        assert_eq!(status, StatusCode::OK, "body: {body}");
        let replay: serde_json::Value = serde_json::from_str(&body).expect("replay json");
        assert_eq!(replay["duplicate"], true);
        assert_eq!(replay["commandId"], command_id);
        assert_eq!(replay["messageId"], message_id);
    }

    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/codex-worker").await;
    assert_eq!(inbox_status, StatusCode::OK, "body: {inbox_body}");
    let inbox: serde_json::Value = serde_json::from_str(&inbox_body).expect("inbox json");
    assert_eq!(inbox["dm"].as_array().expect("dm array").len(), 1);

    let (outbox_status, outbox_body) = get(app, "/api/matrix/outbox?from_seq=0").await;
    assert_eq!(outbox_status, StatusCode::OK, "body: {outbox_body}");
    let outbox: serde_json::Value = serde_json::from_str(&outbox_body).expect("outbox json");
    // The echo filter drops `source == "matrix"` events, so the Matrix-sourced
    // inbound echo must not be replayed back at Matrix even once.
    assert_eq!(outbox["events"].as_array().expect("events").len(), 0);
}

#[tokio::test]
async fn daemon_router_matrix_inbound_second_open_run_request_is_a_conflict() {
    let (app, _dir) = empty_router().await;
    let (agent_status, _) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({ "name": "codex-worker", "runtime": "codex" }),
    )
    .await;
    assert_eq!(agent_status, StatusCode::OK);
    let (room_status, _) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!ops:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK);

    let run_request = serde_json::json!({
        "label": "build",
        "owner": "alice",
        "assignee": "codex-worker",
        "description": "run the build"
    });

    let (first_status, first_body) = post(
        app.clone(),
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$run-1",
            "roomId": "!ops:matrix.test",
            "senderMxid": "@alice:matrix.test",
            "body": "run the build",
            "runRequest": run_request
        }),
    )
    .await;
    assert_eq!(first_status, StatusCode::CREATED, "body: {first_body}");

    let (clash_status, clash_body) = post(
        app,
        "/api/matrix/inbound",
        serde_json::json!({
            "eventId": "$run-2",
            "roomId": "!ops:matrix.test",
            "senderMxid": "@alice:matrix.test",
            "body": "Run the build",
            "runRequest": run_request
        }),
    )
    .await;
    assert_eq!(clash_status, StatusCode::CONFLICT, "body: {clash_body}");
}
```

- [ ] **Step 10: Run the daemon tests**

Run: `cargo nextest run -p agentd-bin --test daemon_http -E 'test(matrix)'`
Expected: PASS, including the pre-existing `daemon_router_matrix_inbound_agent_dm_persists_source_metadata_and_dedupes_event`.
Run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS.

- [ ] **Step 11: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
cargo clippy --all-targets -p agentd-surface -- -D warnings
cargo clippy --all-targets -p agentd-bin -- -D warnings
git add crates/agentd-store/src/message_repo.rs crates/agentd-store/src/relay_repo.rs \
        crates/agentd-store/src/matrix_bridge_repo.rs crates/agentd-store/tests/matrix_bridge.rs \
        crates/agentd-surface/src/host.rs crates/agentd-surface/src/http.rs \
        crates/agentd-surface/src/test_support.rs crates/agentd-bin/src/host.rs \
        crates/agentd-bin/tests/daemon_http.rs
git commit -m "feat(matrix): accept inbound events, commands, inbox and outbox in one transaction"
```

---

## Task 6: Idempotent command → run handoff on the maintenance tick

**Files:**
- Modify: `crates/agentd-store/src/matrix_bridge_repo.rs` (`bind_command_run`)
- Create: `crates/agentd-store/src/matrix_command_dispatch.rs` (the sweep)
- Modify: `crates/agentd-store/src/lib.rs` (`pub mod matrix_command_dispatch;`)
- Modify: `crates/agentd-bin/src/daemon.rs` (`worker_fleet_tick`)
- Test: `crates/agentd-store/tests/matrix_bridge.rs`, `crates/agentd-bin/tests/daemon_http.rs`

**Interfaces:**
- Consumes: Task 4's `list_accepted_commands`, `get_command`, `MatrixCommandRecord` (including its `run_request_json` field) and `MatrixCommandRunPlan`; Task 1's `advance_active_graphs`; existing `agent_chat_task_graph_repo::{create_graph, CreateAgentChatTaskGraph, AgentChatTaskGraphNodeInput}`.
- Produces:
  - `matrix_bridge_repo::bind_command_run(pool, command_id: &str, run_id: &str, expected_version: i64) -> Result<MatrixCommandRecord, StoreError>` — CAS `accepted` → `running` with `run_id`.
  - `matrix_bridge_repo::matrix_command_graph_id(command_id: &str) -> String` — `graph_{command_id}`.
  - `matrix_command_dispatch::dispatch_accepted_commands(pool: &SqlitePool) -> Result<u64, StoreError>` — returns how many commands were bound to a run this sweep.

This task adds **no migration**: `run_request_json` is already a column of `0029` (Task 4) and is already written by `accept_inbound_event` (Task 5). Schema version stays **29**.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/matrix_bridge.rs`:

```rust
#[tokio::test]
async fn dispatching_accepted_commands_creates_exactly_one_run_across_replays() {
    let (store, _dir) = open_temp().await;
    matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text("!ops:matrix.test"),
            project_id: None,
            group_name: None,
            agent_name: Some(text("codex-worker")),
            trusted: true,
            trust_reason: text("managed"),
            inviter_mxid: None,
        },
    )
    .await
    .expect("upsert room");

    let accepted = matrix_bridge_repo::accept_inbound_event(
        store.pool(),
        matrix_bridge_repo::MatrixInboundAcceptance {
            command: matrix_bridge_repo::MatrixCommandInput {
                event_id: text("$run-1"),
                room_id: text("!ops:matrix.test"),
                project_id: Some(text("project-ops")),
                sender_mxid: text("@alice:matrix.test"),
                route: text("agent"),
                body: text("run the build"),
                open: true,
                run_request: Some(matrix_bridge_repo::MatrixCommandRunPlan {
                    label: text("build"),
                    owner: text("alice"),
                    assignee: text("codex-worker"),
                    description: text("run the build"),
                }),
            },
            direct: None,
            group: None,
            relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
        },
    )
    .await
    .expect("accept command");
    assert_eq!(accepted.command.status, "accepted");
    assert!(accepted.command.run_id.is_none());

    let first = agentd_store::matrix_command_dispatch::dispatch_accepted_commands(store.pool())
        .await
        .expect("first dispatch");
    assert_eq!(first, 1);

    // Replay the sweep as many times as a restart loop would. `create_graph`
    // conflicts on the deterministic id, the sweep re-reads and binds, and no
    // second graph is ever created.
    for _ in 0..3 {
        let again = agentd_store::matrix_command_dispatch::dispatch_accepted_commands(store.pool())
            .await
            .expect("replayed dispatch");
        assert_eq!(again, 0, "a bound command is never dispatched twice");
    }

    let graphs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat_task_graphs")
        .fetch_one(store.pool())
        .await
        .expect("count graphs");
    assert_eq!(graphs, 1, "zero duplicate accepted executions");

    let command = matrix_bridge_repo::get_command(store.pool(), &accepted.command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(command.status, "running");
    assert_eq!(
        command.run_id.as_deref(),
        Some(
            matrix_bridge_repo::matrix_command_graph_id(&accepted.command.command_id).as_str()
        )
    );
}

#[tokio::test]
async fn binding_a_command_run_rejects_a_stale_version() {
    let (store, _dir) = open_temp().await;
    matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text("!ops:matrix.test"),
            project_id: None,
            group_name: None,
            agent_name: Some(text("codex-worker")),
            trusted: true,
            trust_reason: text("managed"),
            inviter_mxid: None,
        },
    )
    .await
    .expect("upsert room");

    let accepted = matrix_bridge_repo::accept_inbound_event(
        store.pool(),
        matrix_bridge_repo::MatrixInboundAcceptance {
            command: matrix_bridge_repo::MatrixCommandInput {
                event_id: text("$run-1"),
                room_id: text("!ops:matrix.test"),
                project_id: None,
                sender_mxid: text("@alice:matrix.test"),
                route: text("agent"),
                body: text("run the build"),
                open: true,
                run_request: None,
            },
            direct: None,
            group: None,
            relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
        },
    )
    .await
    .expect("accept command");

    matrix_bridge_repo::bind_command_run(
        store.pool(),
        &accepted.command.command_id,
        "graph_one",
        accepted.command.record_version,
    )
    .await
    .expect("bind run");

    let stale = matrix_bridge_repo::bind_command_run(
        store.pool(),
        &accepted.command.command_id,
        "graph_two",
        accepted.command.record_version,
    )
    .await
    .expect_err("a second bind at the same version must conflict");
    let message = stale.to_string();
    assert!(message.contains("record version mismatch"), "{message}");
    assert!(!message.ends_with("changed concurrently"), "{message}");
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentd-store --test matrix_bridge -E 'test(command_run) or test(dispatching_accepted)'`
Expected: FAIL — `could not find 'matrix_command_dispatch' in 'agentd_store'`.

- [ ] **Step 3: Write `bind_command_run` and the graph id**

Append to `crates/agentd-store/src/matrix_bridge_repo.rs`:

```rust
/// The deterministic task-graph id for one command's run.
#[must_use]
pub fn matrix_command_graph_id(command_id: &str) -> String {
    format!("graph_{}", command_id.trim())
}

/// Bind one accepted command to its run under compare-and-set.
///
/// # Errors
/// [`StoreError::Conflict`] when the command is no longer at
/// `expected_version` or is no longer `accepted` — which is exactly what makes
/// a replayed dispatch sweep a no-op instead of a second execution.
pub async fn bind_command_run(
    pool: &SqlitePool,
    command_id: &str,
    run_id: &str,
    expected_version: i64,
) -> Result<MatrixCommandRecord, StoreError> {
    let command_id = required(command_id.to_string(), "matrix command id required")?;
    let run_id = required(run_id.to_string(), "matrix command run id required")?;
    let updated = sqlx::query(
        "UPDATE matrix_commands \
         SET run_id = ?, status = 'running', record_version = record_version + 1, updated_at = ? \
         WHERE command_id = ? AND record_version = ? AND status = 'accepted' AND run_id IS NULL",
    )
    .bind(&run_id)
    .bind(now_unix())
    .bind(&command_id)
    .bind(expected_version)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{command_id}' record version mismatch"
        )));
    }
    get_command(pool, &command_id)
        .await?
        .ok_or_else(|| StoreError::Invariant(format!("matrix command '{command_id}' is missing")))
}
```

- [ ] **Step 4: Write the dispatch sweep**

Create `crates/agentd-store/src/matrix_command_dispatch.rs`:

```rust
//! Turn accepted Matrix commands into runs, idempotently.
//!
//! Run creation deliberately sits outside the inbound transaction: creating a
//! task graph advances it, which dispatches messages and enqueues execution
//! rows, and none of that belongs inside the request that accepts a Matrix
//! event. Instead the durable command row is the handoff, and this sweep — the
//! same shape and error discipline as `settle_node_executions` — drives it.
//!
//! Idempotency has two independent guards, which is what makes restart/replay
//! produce zero duplicate accepted executions: the graph id is derived from
//! the canonical `command_id`, so a replayed sweep hits `create_graph`'s
//! duplicate-id `Conflict` rather than creating a second graph; and the
//! command→run bind is a compare-and-set that only fires on an `accepted` row
//! with no `run_id`.

use std::collections::BTreeMap;

use sqlx::SqlitePool;

use crate::agent_chat_task_graph_repo;
use crate::error::StoreError;
use crate::matrix_bridge_repo::{
    self, MatrixCommandRecord, MatrixCommandRunPlan, matrix_command_graph_id,
};

/// Create the run for every accepted command that has none yet.
///
/// Returns how many commands were bound to a run. One command's failure is
/// isolated and logged: this runs on the maintenance tick, where a single bad
/// command must not stop the sweep or the loop.
pub async fn dispatch_accepted_commands(pool: &SqlitePool) -> Result<u64, StoreError> {
    let commands = matrix_bridge_repo::list_accepted_commands(pool).await?;
    let mut dispatched = 0_u64;
    for command in commands {
        match dispatch_one(pool, &command).await {
            Ok(true) => dispatched += 1,
            Ok(false) => {}
            Err(error) => {
                tracing::warn!(
                    command_id = command.command_id.as_str(),
                    %error,
                    "dispatching accepted Matrix command failed this tick"
                );
            }
        }
    }
    Ok(dispatched)
}

async fn dispatch_one(
    pool: &SqlitePool,
    command: &MatrixCommandRecord,
) -> Result<bool, StoreError> {
    let Some(plan_json) = command.run_request_json.as_deref() else {
        // Accepted with no run plan: nothing to create. Settle it so it stops
        // holding the open-dedup slot for its room and project.
        settle_without_run(pool, command).await?;
        return Ok(false);
    };
    let plan: MatrixCommandRunPlan = serde_json::from_str(plan_json)?;
    let graph_id = matrix_command_graph_id(&command.command_id);

    let mut nodes = BTreeMap::new();
    nodes.insert(
        "run".to_string(),
        agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
            id: None,
            assignee: Some(plan.assignee.clone()),
            role: None,
            capability: None,
            description: plan.description.clone(),
            depends_on: Vec::new(),
            condition: None,
            execution: None,
        },
    );

    match agent_chat_task_graph_repo::create_graph(
        pool,
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.clone()),
            owner: plan.owner.clone(),
            label: plan.label.clone(),
            nodes,
        },
    )
    .await
    {
        Ok(_) => {}
        // The graph already exists: a previous sweep created it and crashed
        // before binding. Proceed to the bind; do not create a second graph.
        Err(StoreError::Conflict(message)) if message.starts_with("task graph already exists") => {}
        Err(error) => return Err(error),
    }

    // The graph is deliberately left `pending`: `advance_active_graphs` on the
    // same tick dispatches it, which is the one place graph advance lives.
    matrix_bridge_repo::bind_command_run(
        pool,
        &command.command_id,
        &graph_id,
        command.record_version,
    )
    .await?;
    Ok(true)
}

async fn settle_without_run(
    pool: &SqlitePool,
    command: &MatrixCommandRecord,
) -> Result<(), StoreError> {
    let updated = sqlx::query(
        "UPDATE matrix_commands \
         SET status = 'settled', record_version = record_version + 1, updated_at = ? \
         WHERE command_id = ? AND record_version = ? AND status = 'accepted'",
    )
    .bind(crate::util::now_unix())
    .bind(&command.command_id)
    .bind(command.record_version)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "matrix command '{}' record version mismatch",
            command.command_id
        )));
    }
    Ok(())
}
```

Register it in `crates/agentd-store/src/lib.rs` next to `pub mod matrix_bridge_repo;` (line 32):

```rust
pub mod matrix_command_dispatch;
```

- [ ] **Step 5: Run the store test**

Run: `cargo nextest run -p agentd-store --test matrix_bridge`
Expected: PASS.

- [ ] **Step 6: Wire the sweep into the maintenance tick**

In `crates/agentd-bin/src/daemon.rs`, in `worker_fleet_tick`, **before** Task 1's `advance_active_graphs` block (so a command's graph is created and advanced in the same tick):

```rust
    // Accepted Matrix commands become runs here, not inside the inbound
    // request: the durable command row is the handoff, and both the graph id
    // and the command→run bind are idempotent, so a replayed sweep after a
    // restart creates nothing new.
    if let Err(error) = agentd_store::matrix_command_dispatch::dispatch_accepted_commands(
        native_worker.store().pool(),
    )
    .await
    {
        tracing::warn!(%error, "dispatching accepted Matrix commands failed this tick");
    }
```

- [ ] **Step 7: Write the daemon-level exit-criterion test**

Append to `crates/agentd-bin/tests/daemon_http.rs`. This asserts the M4 exit criterion directly against the store the router writes into, driving the sweep the way the tick does:

```rust
#[tokio::test]
async fn matrix_run_request_produces_exactly_one_execution_across_restart_and_replay() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect");
    let pool = store.pool().clone();
    let host = ProductionRunHost::new(
        store,
        Box::new(SharedBackend(Arc::new(FakeBackend::new()))),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    let app = daemon::build_router(Arc::new(host));

    let (agent_status, _) = post(
        app.clone(),
        "/api/agents",
        serde_json::json!({ "name": "codex-worker", "runtime": "codex" }),
    )
    .await;
    assert_eq!(agent_status, StatusCode::OK);
    let (room_status, _) = post(
        app.clone(),
        "/api/matrix/rooms",
        serde_json::json!({
            "roomId": "!ops:matrix.test",
            "agent": "codex-worker",
            "trusted": true,
            "trustReason": "managed"
        }),
    )
    .await;
    assert_eq!(room_status, StatusCode::OK);

    let inbound = serde_json::json!({
        "eventId": "$run-restart",
        "roomId": "!ops:matrix.test",
        "senderMxid": "@alice:matrix.test",
        "body": "run the build",
        "runRequest": {
            "label": "build",
            "owner": "alice",
            "assignee": "codex-worker",
            "description": "run the build"
        }
    });

    let (status, body) = post(app.clone(), "/api/matrix/inbound", inbound.clone()).await;
    assert_eq!(status, StatusCode::CREATED, "body: {body}");

    // Tick, restart (replayed inbound), tick, tick.
    for round in 0..3 {
        agentd_store::matrix_command_dispatch::dispatch_accepted_commands(&pool)
            .await
            .expect("dispatch accepted commands");
        agentd_store::agent_chat_task_graph_repo::advance_active_graphs(&pool)
            .await
            .expect("advance active graphs");
        let (replay_status, replay_body) =
            post(app.clone(), "/api/matrix/inbound", inbound.clone()).await;
        assert_eq!(
            replay_status,
            StatusCode::OK,
            "round {round} body: {replay_body}"
        );
    }

    let graphs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat_task_graphs")
        .fetch_one(&pool)
        .await
        .expect("count graphs");
    assert_eq!(graphs, 1, "restart/replay produced a duplicate execution");
    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM direct_messages")
        .fetch_one(&pool)
        .await
        .expect("count messages");
    // One inbox message for the Matrix event, one task_graph_dispatch message
    // for the single node the run advanced.
    assert_eq!(messages, 2, "restart/replay produced a duplicate message");
}
```

Add `use agentd_store::SqliteStore;` if it is not already imported (it is, at line 19) and `use agentd_bin::daemon::build_router` via the existing `daemon::` path. If `build_router` takes an auth argument in this crate, use the same constructor `empty_router_with_backend` uses at line 1361.

- [ ] **Step 8: Run the tests**

Run: `cargo nextest run -p agentd-bin --test daemon_http -E 'test(matrix)'`
Expected: PASS.
Run: `cargo nextest run -p agentd-bin --test m3_coordination_e2e`
Expected: PASS.
Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentd-store -- -D warnings
cargo clippy --all-targets -p agentd-bin -- -D warnings
git add crates/agentd-store/src/matrix_bridge_repo.rs \
        crates/agentd-store/src/matrix_command_dispatch.rs \
        crates/agentd-store/src/lib.rs crates/agentd-store/tests/matrix_bridge.rs \
        crates/agentd-bin/src/daemon.rs crates/agentd-bin/tests/daemon_http.rs
git commit -m "feat(matrix): create command runs idempotently from the maintenance tick"
```

---

## Task 7: Parity and roadmap evidence for p263/p264

**Files:**
- Modify: `docs/parity/agent-chat-capability-map.md` (the `matrix_bridge` decision cell, line 33)
- Modify: `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md` (append after the p262 entry, ~line 715)
- Modify: `crates/agentctl/tests/parity_cli.rs` (append one test)

**Interfaces:**
- Consumes: everything Tasks 1–6 built, by name.
- Produces: no code interfaces. `matrix_bridge` and `remote_relay` both stay `partial`.

- [ ] **Step 1: Write the failing contract test**

Append to `crates/agentctl/tests/parity_cli.rs`, mirroring `parity_capability_map_records_p262_matrix_joingroup_progress`:

```rust
#[test]
fn parity_capability_map_records_p264_matrix_gateway_core_progress() {
    let rows = parity_rows();
    let matrix = rows
        .iter()
        .find(|row| row.capability == "matrix_bridge")
        .expect("matrix_bridge row");
    let roadmap = std::fs::read_to_string(roadmap_path()).expect("read roadmap");
    let store_source =
        std::fs::read_to_string(repo_root().join("crates/agentd-store/src/matrix_bridge_repo.rs"))
            .expect("read matrix bridge repo source");
    let dispatch_source = std::fs::read_to_string(
        repo_root().join("crates/agentd-store/src/matrix_command_dispatch.rs"),
    )
    .expect("read matrix command dispatch source");

    assert_eq!(matrix.status, "partial");
    for expected in [
        "p263",
        "p264",
        "matrix_gateway_cursors",
        "matrix_commands",
        "command_id",
        "BEGIN IMMEDIATE",
        "admin commands",
        "Matrix media",
        "real homeserver",
        "service packaging",
        "cutover",
        "rollback",
        "token rotation",
        "bridge operations",
        "dashboard/operator visibility",
    ] {
        assert!(
            matrix.decision.contains(expected),
            "matrix bridge decision should mention {expected}: {}",
            matrix.decision
        );
    }

    for expected in [
        "p263",
        "p264",
        "durable cursor",
        "canonical `command_id`",
        "Matrix bridge remains partial",
    ] {
        assert!(
            roadmap.contains(expected),
            "roadmap should mention {expected}: {roadmap}"
        );
    }

    for expected in [
        "advance_gateway_cursor",
        "matrix_command_id",
        "accept_inbound_event",
        "bind_command_run",
    ] {
        assert!(
            store_source.contains(expected),
            "matrix bridge repo should mention {expected}"
        );
    }
    assert!(dispatch_source.contains("dispatch_accepted_commands"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo nextest run -p agentctl --test parity_cli -E 'test(p264)'`
Expected: FAIL — the decision cell has no `p263`.

- [ ] **Step 3: Append the parity evidence**

In `docs/parity/agent-chat-capability-map.md`, in the `matrix_bridge` row's decision cell, insert immediately before the trailing `This remains partial until …` sentence:

> p263 adds the `AgentdMatrixGateway`-owned durable inbound cursor: migration `0028_matrix_gateway_cursors` stores a per-gateway `sync_token`/`last_event_id` under `record_version` compare-and-set in the daemon database, `GET`/`PUT /api/matrix/gateway/cursor` serve it to the remote bridge over HTTP only, and `BridgeRuntime::run_once` seeds from the daemon cursor and advances it after every inbound batch is durably accepted — replacing a state in which no inbound cursor existed anywhere and a restarted gateway re-delivered its initial-sync timeline. p264 adds the transactional command inbox/run/outbox handoff: migration `0029_matrix_commands` stores a canonical `command_id` derived deterministically from `(room_id, event_id)` with a partial unique room/project dedup constraint over open commands, `accept_inbound_event` writes the processed-event row, the command row, the inbox message under a `command_id`-derived deterministic message id, and the outbox relay event inside one `BEGIN IMMEDIATE`, and `dispatch_accepted_commands` creates each command's run under a deterministic graph id and binds it with a compare-and-set from the daemon maintenance tick, so restart and replay produce zero duplicate accepted executions.

Then extend the trailing sentence's outstanding list so it still names what is undone, adding `trusted inviter and ignored-sender enforcement, command normalization, attachment ingest, Robrix views` to the existing enumeration.

- [ ] **Step 4: Append the roadmap entry**

In `docs/plans/2026-07-08-agent-chat-replacement-roadmap.md`, after the p262 paragraph (ending at line 715):

```markdown
Update 2026-07-29: p263 and p264 add the M4 Plan A Matrix gateway core. p263
gives the gateway an agentd-owned durable cursor: migration
`0028_matrix_gateway_cursors` holds a per-gateway Matrix sync token and last
accepted event id under compare-and-set in the daemon database, the remote
bridge reads and advances it through `GET`/`PUT /api/matrix/gateway/cursor`
and never opens the daemon database, and `BridgeRuntime` seeds from it each
iteration and advances it only after a whole inbound batch is durably
accepted. p264 makes the command handoff transactional: migration
`0029_matrix_commands` stores a canonical `command_id` derived from
`(room_id, event_id)` under a partial unique room/project dedup constraint
covering open commands, one `BEGIN IMMEDIATE` writes the processed-event row,
the command row, the inbox message under a deterministic `command_id`-derived
id, and the Matrix outbox event together, and the daemon maintenance tick
creates each accepted command's run under a deterministic task-graph id and
binds it with a compare-and-set — so restart and replay produce zero duplicate
accepted executions. The same tick now re-advances active task graphs, closing
the M3 carry-over in which a graph whose creation-time advance failed was never
re-driven. Matrix bridge remains partial: p264 still does not implement trusted
inviter or ignored-sender enforcement, appservice loop suppression, command
normalization, attachment ingest, Robrix project/run/task/artifact views, admin
commands, Matrix media, real homeserver evidence, service packaging, cutover,
rollback, token rotation, bridge operations, or dashboard/operator visibility.
```

- [ ] **Step 5: Run the contract suites**

Run: `cargo nextest run -p agentctl --test parity_cli`
Expected: PASS — every pre-existing `matrix_bridge` test still passes because the status cell is unchanged and only new substrings were added.
Run: `cargo nextest run -p agentctl --test worktree_reconciliation_contract`
Expected: PASS — `rows["matrix_bridge"][4] == "partial"` still holds.
Run: `cargo nextest run -p agentctl --test enterprise_project_authority_contract`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy --all-targets -p agentctl -- -D warnings
git add docs/parity/agent-chat-capability-map.md \
        docs/plans/2026-07-08-agent-chat-replacement-roadmap.md \
        crates/agentctl/tests/parity_cli.rs
git commit -m "docs(parity): record p263/p264 Matrix gateway core evidence"
```

---

## Verification (run after Task 7, one gate at a time)

Never two `nextest` invocations concurrently, never a workspace-wide run.

```bash
cargo nextest run -p agentd-store --test matrix_bridge
cargo nextest run -p agentd-store --test migration
cargo nextest run -p agentd-store --test operational_doctor
cargo nextest run -p agentd-store --test agent_chat_task_graphs
cargo nextest run -p agentd-store --test messages
cargo nextest run -p agentd-matrix --test http_backend
cargo nextest run -p agentd-matrix --test bridge_runtime
cargo nextest run -p agentd-matrix --test client_bridge_once
cargo nextest run -p agentd-surface --test http
cargo nextest run -p agentd-bin --test daemon_http
cargo nextest run -p agentd-bin --test m3_coordination_e2e
cargo nextest run -p agentctl --test parity_cli
cargo nextest run -p agentctl --test worktree_reconciliation_contract
```

**Exit-criterion evidence for M4:** `matrix_run_request_produces_exactly_one_execution_across_restart_and_replay` (Task 6) is the direct proof that restart/replay produces zero duplicate accepted executions; `daemon_router_matrix_inbound_replay_never_creates_a_second_message` (Task 5) is the proof for the inbox and outbox sides.

## Release notes for the merge

- **Schema version moves 27 → 29.** Two migrations, `0028_matrix_gateway_cursors` and `0029_matrix_commands`. Both are pure additions; no existing table is altered.
- **`POST /api/matrix/inbound` can now return 409.** A second *open* command for the same `(room, project, payload)` is rejected. Plain chat messages never take that path.
- **`POST /api/matrix/inbound` responses gain `commandId`.** Existing consumers are unaffected; the field is additive.
- **Matrix inbox message ids are now deterministic** (`msg_mxc_<hash>`) instead of ULIDs. Anything that assumed a ULID shape for Matrix-sourced messages needs checking.
- **New routes:** `GET`/`PUT /api/matrix/gateway/cursor`, both operator-bearer.
- **The maintenance tick does two more things per iteration:** dispatch accepted Matrix commands, and re-advance active task graphs. Both are error-isolated and warn-logged.
- **Still open (Plan B/C):** trusted inviter and ignored-sender enforcement, appservice loop suppression, command normalization (and with it the real `dedup_key` derivation), attachment ingest, Robrix views. The bridge does not yet populate `runRequest`, so the run path is exercised by tests and by operators posting it directly until Plan B lands.
- **Follow-up tickets:** thread the stored `sync_token` into `SyncSettings::token(...)` behind `matrix-sdk-adapter` with real-homeserver evidence; make the outbox cursor's `bridgeId` a parameter instead of the hardcoded `"matrix-bridge"`; add `DurableSchedulerPort::cancel` so a settled command's graph can be cancelled.
