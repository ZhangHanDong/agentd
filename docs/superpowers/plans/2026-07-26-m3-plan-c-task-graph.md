# M3 Plan C — Task Graph Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make task-graph coordination genuinely agentd-owned — settled nodes become immutable, concurrent node results stop losing each other, a graph node can be executed by a native worker through the M2 durable queue instead of only by messaging a human-shaped agent, imported agent-chat graphs actually run — and prove the M3 exit gate end to end: agents register, message, and run a task graph with no agent-chat process and no tmux in the path.

**Architecture:** The live `/api/tasks` and `/api/task-graphs` surfaces already exist (p226/p227) and are route-for-route identical to agent-chat's, so this plan adds **no new HTTP routes**. The work is in the store: `agent_chat_task_graph_repo` today reads a whole graph, mutates it in memory, and blind-overwrites a single `raw_json` blob, with no concurrency guard and no rule preventing a settled node from being rewritten; and its only dispatch actions are "insert a direct message" or "reserve through the p228 tmux-era pool scheduler". Two schema tasks fix that: migration `0026` adds a `record_version` to the graph row so every write is a guarded compare-and-set, and migration `0027` adds `task_graph_node_executions`, the durable link between a graph node and an `execution_task_queue` row, so a node carrying an `execution` spec is enqueued for a native worker and settled from that queue row's terminal status by the daemon maintenance tick.

**Tech Stack:** Rust 2024, tokio, axum 0.7-style `Router`/extractors, sqlx + SQLite, serde/serde_json, `cargo nextest`, `tempfile`, `tower::ServiceExt::oneshot`, `http_body_util::BodyExt`.

## Global Constraints

- **Error classification:** `Invalid` → 400, `NotFound` → 404, `Conflict` → 409, and only `Unavailable` is retryable → 503. In this plan's layer the mapping is by string convention: `StoreError::Invariant` → `CoreError::Invariant` → 400, `StoreError::Conflict` → `CoreError::Store("conflict: …")` → 409 (Task 1 adds that branch).
- **Multi-statement mutations** run inside `BEGIN IMMEDIATE` with `rows_affected` guards on every write. Where a full `BEGIN IMMEDIATE` region is impossible (Task 2 explains exactly why for the graph-advance path), the write MUST still carry a `rows_affected` guard against a version predicate.
- **Liveness columns** (`status`, `offline_reason`, `last_seen_at` on `agents`) are owned only by heartbeat / start / offline / sweep. Nothing in this plan may write them.
- **`agentd-surface` stays store-free.** It depends on `agentd-core` ports and its own `RunHost` trait only — never on `agentd-store`. Every surface type is a hand-mirrored struct; when a store type gains a field that must reach the wire, the mirror in `agentd-surface/src/host.rs` and the mapping in `agentd-bin/src/host.rs` change too.
- **Any schema change = a new migration bumping `schema_meta.version`**, with the `crates/agentd-store/tests/migration.rs` version assertions and the `crates/agentd-store/tests/operational_doctor.rs` schema-version assertion updated in the **same task**. This plan has exactly two: `0026_task_graph_record_version.sql` → version **26** (Task 2) and `0027_task_graph_node_executions.sql` → version **27** (Task 3). Do not fold them together and do not add a third.
- **Parity status cells must NOT change without updating the contract tests in the same commit.** The suites are `crates/agentctl/tests/parity_cli.rs`, `crates/agentctl/tests/worktree_reconciliation_contract.rs`, and `crates/agentctl/tests/enterprise_project_authority_contract.rs`. Only `parity_cli.rs` asserts on the `task_graph_coordination` / `migration_shadow_cutover` rows (`parity_capability_map_records_p227_live_task_graph_progress`), and it asserts both rows are `partial` and that each `decision` cell still contains the substrings `p227`, `live`, `/api/task-graphs`, `dispatch`, `scheduler`, `dashboard`, `Matrix`, `remote relay`, `service cutover`, `rollback`, `token provisioning`. **Both rows stay `partial` in this plan.** Task 6 only appends new evidence text and extends the assertion list.
- **Test gates are narrow.** Always use a single `--test <name>` (or `--lib`) gate scoped to one package with `-p`. Never run workspace-wide `cargo nextest run`. Never run two `nextest` invocations concurrently. Avoid multi-package `-p a -p b` combinations.
- **Before every commit:** `cargo fmt --all` then `cargo clippy --all-targets -- -D warnings` (scoped to the touched packages with `-p` where practical).

---

## Gap Analysis: what p225–p234 and M2 already cover

Read this before starting. It is why the plan has six tasks and no CRUD task.

**CRUD is already at full route parity — do not add or rename a route.** agent-chat's `backend-v2.js` exposes exactly nine task routes and five task-graph routes:

| agent-chat | agentd |
|---|---|
| `POST/GET /api/tasks`, `GET/PATCH/DELETE /api/tasks/:id`, `PATCH /api/tasks/:id/execution`, `POST /api/tasks/:id/{accept,transition,comments}` | all nine present in `crates/agentd-surface/src/http.rs:157-163` |
| `POST/GET /api/task-graphs`, `GET/DELETE /api/task-graphs/:id`, `PATCH /api/task-graphs/:id/nodes/:nodeId` | all five present in `crates/agentd-surface/src/http.rs:165-176` |

The auth shape matches too: graph create/delete are operator-bearer, node patch is agent-token-by-assignee (`require_task_graph_node_assignee_token`), exactly as agent-chat's `requireBearer` / `requireAgentToken(_tokenFromNodeAssignee)`. Task status values (`created`/`accepted`/`in_progress`/`blocked`/`done`), priorities, granularities, the 100-comment cap, and the transition guard already live in `agent_chat_task_repo.rs`. **There is no CRUD gap. Do not write a CRUD task.**

**Dispatch is the real gap, and it does not touch M2 at all.** `agent_chat_task_graph_repo::advance_graph_record` has exactly two dispatch actions for a ready node:

1. no `role` → build a `task_graph_dispatch` direct message and `message_repo::insert_direct_message` it to `node.assignee`;
2. `role` set → `agent_scheduler_repo::dispatch` (the p228/p229 reservation pool), which resolves a registry agent and then *also* sends that same direct message.

Neither path touches `execution_task_queue`, `SqliteDurableScheduler`, `task_runs`, a lease, or a worker. `grep -rn "execution_task_queue\|task_runs" crates/agentd-store/src/agent_chat_task_graph_repo.rs` returns nothing. So today a task graph can only be *executed* by something that reads an inbox and posts a `task_graph_result` message back; agentd coordinates the conversation and owns none of the execution. M2 Plan B delivered `dispatch_task_to_fleet` (spec + enqueue + native worker pull, no tmux) but wired it only to the workflow-engine direction, never to graph nodes.

**Decision on scope (the brief asks for it explicitly): the node ↔ `execution_task_queue` link is built in M3, not deferred to M5.** M2's own outcome statement is "the daemon durably queues a task graph and dispatches its nodes to whichever workers are online", and M3 item 3 is "coordination semantics driven by agentd" — a graph whose nodes can only be executed by an external reader is not that. M5 is the per-project cutover/rollback *state machine*; plumbing a node to the queue is not cutover. Tasks 3 and 4 build it as an **additive, opt-in node mode**: a node with an `execution` spec goes to the durable queue and is settled by the daemon; a node without one keeps the exact p227 message behaviour, byte for byte. Nothing existing changes shape.

**Node semantics have three concrete holes, all reachable from the live HTTP surface.**

1. *Settled nodes are mutable.* `apply_node_patch` validates only status *membership*, so `PATCH …/nodes/a {"status":"pending"}` resurrects a `complete` node, and a late duplicate `task_graph_result` message rewrites a node's result. agent-chat has the same hole — this is a deliberate agentd improvement, not a parity fix.
2. *A non-active graph still accepts node writes.* `update_node_and_advance` applies the patch and *then* calls `advance_graph_record`, which early-returns for a non-`active` graph — but only after `upsert_graph` persists the patch. So a node of a `cancelled` (deleted) graph can still be mutated.
3. *Concurrent writes lose each other.* The whole graph is one `raw_json` blob, written by a read-modify-write over the pool with no transaction and no version predicate. Two sibling nodes completing at the same moment — the normal fan-out case this feature exists for — reliably drop one result.

**Failure handling is complete for the message path and must not be extended.** `advance_graph_record` already marks downstream nodes `failed` with `"dependency failed"`, already skips on unmet conditions, already flips the graph to `failed`/`complete` when every node is terminal, and `delete_graph` already cancels non-terminal nodes. There is **no per-node retry**, and agent-chat has none either. Do not add one: for native nodes, M2's queue already owns retry (`max_attempts`, requeue on lease expiry, dead-letter on exhaustion), and a node failing is a *semantic* result that must stay terminal.

**Error classification is wrong on the existing surface.** `StoreError::Conflict` (e.g. `create_graph`'s duplicate-id) becomes `CoreError::Store("conflict: …")`, and `task_error_response` only special-cases the `"invariant violated: "` prefix — so a duplicate graph id returns **500** today. Task 1 fixes it, which is also what makes Tasks 1–4's new conflicts observable as 409.

**Migration is preserved but not *live*.** p225 imports `task_graphs.json` by storing agent-chat's JSON **verbatim** into `agent_chat_task_graphs.raw_json` (`upsert_imported_task_graph`), and the import test asserts only that the raw row contains `"nodes"`. But every read goes through `row_to_graph`, which does a typed `serde_json::from_str::<AgentChatTaskGraphRecord>` requiring `owner`, `label`, `status`, `createdAt`, `updatedAt` and, per node, `id`, `assignee`, `description`, `status`. An agent-chat graph missing any of those is unreadable — and because `list_graphs` collects with `collect::<Result<Vec<_>,_>>()?`, **one bad row makes `GET /api/task-graphs` 500 for every graph**. No test anywhere asserts that an imported graph can be read, let alone advanced. Task 5 closes that: import normalizes into the live shape, and listing degrades per-row instead of wholesale.

**What M2 gives this plan for free.** `SqliteDurableScheduler::enqueue` is request-id idempotent (exact replay returns the row, divergent payload is a `Conflict`); `acquire` is capacity- and capability-aware and terminalizes rows whose task closed; `reconcile` maps a terminal lease onto `completed` / `cancelled` / `queued` (requeue) / `dead_letter` and is already called every tick from `worker_fleet_tick`. Task 4 reads those terminal queue statuses and does not reimplement any of it.

---

## Non-Goals (explicitly out of scope for this plan)

- **The M3B hardening tickets** (`deny_unknown_fields` on `InboxQuery`, the `direct_messages.to_agent` / `group_members.group_name` / `group_mention_reads.agent_name` case-collation convention, `agent_error_response` 500-vs-503, the `worker_fleet_http` `respond()` variant→status mapping). None is in task-graph scope; they belong to a later hardening pass. The one classification fix this plan *does* make (Task 1) is in scope only because task-graph conflicts are unobservable without it.
- **Per-node retry, timeout, or backoff on the message dispatch path.** agent-chat has none and M2's queue owns retry for native nodes.
- **Cancelling an already-dispatched native node's queue row when its graph is deleted.** M2 exposes no scheduler cancel primitive (a queue row only reaches `cancelled` via a cancelled lease). Task 4 marks the link settled so the cancelled node is never rewritten, and the in-flight execution is allowed to finish and be discarded. Follow-up ticket: add a `DurableSchedulerPort::cancel` and call it from `delete_graph`.
- **Dashboard/CLI views of graphs, Matrix delivery of graph events, cutover and rollback.** M4/M5.
- **Replacing the p228/p229 reservation pool** (`role`-scheduled nodes, tickets, drain wakeups). It keeps working unchanged; the native path is additive. Retiring it is M6.
- **New HTTP routes.** The surface is already at agent-chat route parity.

---

### Task 1: Settled nodes are immutable, and conflicts are 409

Three changes that together make illegal task-graph mutations observable and refused: a settled node cannot be rewritten, a non-active graph cannot be written at all, and `StoreError::Conflict` finally reaches the wire as 409 instead of 500. The empty-patch rejection is folded in because agent-chat rejects it (`invalid_patch`, "node patch requires status, result, or error") and agentd silently accepts it.

**Files:**
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs` (`update_node_and_advance`, `handle_result_message`, `apply_node_patch`)
- Modify: `crates/agentd-surface/src/http.rs` (`task_error_response`)
- Modify: `crates/agentd-surface/src/test_support.rs` (`FakeRunHost` must mirror the new store behaviour or the HTTP tests cannot see it)
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs`
- Test: `crates/agentd-surface/tests/http.rs`

**Interfaces:**
- Consumes: nothing from earlier tasks.
- Produces: `StoreError::Conflict` from `update_node_and_advance` for a non-active graph or a settled node; `handle_result_message` returns `Ok(None)` (not an error) for a late/duplicate result; `task_error_response` maps a `CoreError::Store` message starting with `"conflict: "` to `409` with the prefix stripped. Tasks 2–4 rely on all three.

- [ ] **Step 1: Write the failing store tests**

Append to `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
#[tokio::test]
async fn node_updates_on_a_cancelled_graph_are_rejected() {
    let (store, _dir) = open_store().await;
    let graph = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_cancelled".to_string()),
            owner: "orchestrator".to_string(),
            label: "Cancelled graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    assert_eq!(graph.status, "active");

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_cancelled")
        .await
        .expect("delete graph")
        .expect("graph present");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_cancelled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect_err("cancelled graphs reject node updates");
    assert!(
        matches!(&error, agentd_store::StoreError::Conflict(message) if message.contains("cancelled")),
        "expected a conflict naming the graph status, got: {error}"
    );
}

#[tokio::test]
async fn settled_nodes_reject_further_updates() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_settled".to_string()),
            owner: "orchestrator".to_string(),
            label: "Settled graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");

    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_settled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect("first completion")
    .expect("graph and node");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_settled",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("pending".to_string()),
            result: None,
            error: None,
        },
    )
    .await
    .expect_err("a settled node cannot be resurrected");
    assert!(
        matches!(&error, agentd_store::StoreError::Conflict(message) if message.contains("already complete")),
        "expected a conflict naming the settled status, got: {error}"
    );

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settled")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["a"].status, "complete");
    assert_eq!(graph.nodes["a"].result, Some(json!({"ok": true})));
}

#[tokio::test]
async fn empty_node_patches_are_rejected() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_empty_patch".to_string()),
            owner: "orchestrator".to_string(),
            label: "Empty patch graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");

    let error = agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_empty_patch",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode::default(),
    )
    .await
    .expect_err("an empty patch is invalid");
    assert!(
        matches!(&error, agentd_store::StoreError::Invariant(message)
            if message.contains("status, result, or error")),
        "expected an invariant naming the required fields, got: {error}"
    );
}

#[tokio::test]
async fn late_duplicate_result_messages_are_ignored() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_late".to_string()),
            owner: "orchestrator".to_string(),
            label: "Late result graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    // `create_graph` only persists the graph; `advance_graph` is what
    // dispatches the root node and assigns its message id.
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_late")
        .await
        .expect("advance graph")
        .expect("graph present");
    let reply_to = graph.nodes["a"]
        .message_id
        .clone()
        .expect("dispatch message id");
    let schema = json!({
        "kind": "task_graph_result",
        "version": 1,
        "payload": {"graphId": "graph_late", "nodeId": "a", "result": {"attempt": 1}}
    });

    let first = agent_chat_task_graph_repo::handle_result_message(
        store.pool(),
        "codex-a",
        Some(&reply_to),
        Some(&schema),
    )
    .await
    .expect("first result")
    .expect("handled");
    assert_eq!(first.status, "complete");

    let second = agent_chat_task_graph_repo::handle_result_message(
        store.pool(),
        "codex-a",
        Some(&reply_to),
        Some(&json!({
            "kind": "task_graph_failed",
            "version": 1,
            "payload": {"graphId": "graph_late", "nodeId": "a", "error": "too late"}
        })),
    )
    .await
    .expect("a late duplicate is not an error");
    assert!(second.is_none(), "late duplicate results are ignored");

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_late")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["a"].status, "complete");
    assert_eq!(graph.nodes["a"].error, None);
}
```

- [ ] **Step 2: Run the store tests to verify they fail**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: FAIL — `node_updates_on_a_cancelled_graph_are_rejected`, `settled_nodes_reject_further_updates`, `empty_node_patches_are_rejected` fail with "expect_err" panics (the calls currently succeed), and `late_duplicate_result_messages_are_ignored` fails because `second` is `Some`.

- [ ] **Step 3: Add the guards in the store**

In `crates/agentd-store/src/agent_chat_task_graph_repo.rs`, replace the head of `update_node_and_advance` (the `let Some(mut graph) = …` / `let Some(node) = …` prologue) with:

```rust
pub async fn update_node_and_advance(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, StoreError> {
    let Some(mut graph) = get_graph(pool, graph_id).await? else {
        return Ok(None);
    };
    // A graph that is no longer active is settled: `advance_graph_record`
    // early-returns for it, so without this guard the patch would still be
    // persisted onto a cancelled/complete graph by the trailing upsert.
    if graph.status != "active" {
        return Err(StoreError::Conflict(format!(
            "task graph '{graph_id}' is {} and no longer accepts node updates",
            graph.status
        )));
    }
    let Some(node) = graph.nodes.get_mut(node_id) else {
        return Ok(None);
    };
    // Settled nodes are immutable: their result has already unlocked (or
    // failed) downstream work, so rewriting one would desynchronize the graph
    // from the decisions already taken on it.
    if node_terminal(&node.status) {
        return Err(StoreError::Conflict(format!(
            "task graph node '{node_id}' is already {}",
            node.status
        )));
    }
    let release_agent = patch
```

(everything from `let release_agent = patch` onwards is unchanged.)

In the same file, add the empty-patch check as the first statement of `apply_node_patch`:

```rust
fn apply_node_patch(
    node: &mut AgentChatTaskGraphNode,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<(), StoreError> {
    if patch.status.is_none() && patch.result.is_none() && patch.error.is_none() {
        return Err(StoreError::Invariant(
            "node patch requires status, result, or error".to_string(),
        ));
    }
    let now = now_text();
```

And in `handle_result_message`, immediately after the existing sender/reply-to ownership check, add the settled short-circuit so a late duplicate is *ignored* rather than turned into a 409 on an otherwise-valid `POST /api/messages`:

```rust
    if node.assignee != from || node.message_id.as_deref() != Some(reply_to.as_str()) {
        return Ok(None);
    }
    // A late or duplicated result for an already-settled node (or a graph that
    // has since been cancelled) is dropped, not rejected: the message itself is
    // legitimate and is still stored, it simply no longer moves the graph.
    if graph.status != "active" || node_terminal(&node.status) {
        return Ok(None);
    }
```

- [ ] **Step 4: Run the store tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS, all tests in the file.

- [ ] **Step 5: Write the failing HTTP tests**

Append to `crates/agentd-surface/tests/http.rs`:

```rust
#[tokio::test]
async fn http_task_graph_duplicate_id_is_a_conflict() {
    let app = app(FakeRunHost::new());

    let created = post(
        app.clone(),
        "/api/task-graphs",
        &chain_graph_body().to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);

    let duplicate = post(app, "/api/task-graphs", &chain_graph_body().to_string()).await;
    assert_eq!(duplicate.status(), StatusCode::CONFLICT);
    let duplicate: Value =
        serde_json::from_str(&body_string(duplicate).await).expect("duplicate json");
    assert!(
        duplicate["error"]
            .as_str()
            .is_some_and(|error| error.contains("already exists")),
        "body: {duplicate}"
    );
}

#[tokio::test]
async fn http_task_graph_node_patch_after_delete_is_a_conflict() {
    let app = app(FakeRunHost::new());

    let created = post(
        app.clone(),
        "/api/task-graphs",
        &chain_graph_body().to_string(),
    )
    .await;
    assert_eq!(created.status(), StatusCode::OK);

    let deleted = delete(app.clone(), "/api/task-graphs/graph_live").await;
    assert_eq!(deleted.status(), StatusCode::OK);

    let patched = patch(
        app,
        "/api/task-graphs/graph_live/nodes/a",
        &json!({"status": "complete", "result": {"ok": true}}).to_string(),
    )
    .await;
    assert_eq!(patched.status(), StatusCode::CONFLICT);
    let patched: Value = serde_json::from_str(&body_string(patched).await).expect("patch json");
    assert!(
        patched["error"]
            .as_str()
            .is_some_and(|error| error.contains("cancelled")),
        "body: {patched}"
    );
}
```

- [ ] **Step 6: Run the HTTP tests to verify they fail**

Run: `cargo nextest run -p agentd-surface --test http http_task_graph`
Expected: FAIL — the duplicate create returns 400 (the fake raises `Invariant`) and the post-delete patch returns 200.

- [ ] **Step 7: Map `conflict:` to 409 and mirror the guards in `FakeRunHost`**

In `crates/agentd-surface/src/http.rs`, add a branch to `task_error_response` above the existing `"invariant violated: "` branch:

```rust
fn task_error_response(e: CoreError) -> Response {
    match e {
        CoreError::Invariant(message) => {
            (StatusCode::BAD_REQUEST, Json(json!({ "error": message }))).into_response()
        }
        // `StoreError::Conflict` arrives as `CoreError::Store("conflict: …")`
        // (see `agentd_store::error`); classify it as 409, not 500.
        CoreError::Store(message) if message.starts_with("conflict: ") => (
            StatusCode::CONFLICT,
            Json(json!({ "error": message.trim_start_matches("conflict: ") })),
        )
            .into_response(),
        CoreError::Store(message) if message.starts_with("invariant violated: ") => (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": message.trim_start_matches("invariant violated: ") })),
        )
            .into_response(),
        other => agent_error_response(other),
    }
}
```

In `crates/agentd-surface/src/test_support.rs`, add the conflict helper next to the other fake task-graph helpers:

```rust
/// Mirror of `agentd_store::error`'s `StoreError::Conflict` → `CoreError`
/// mapping so `FakeRunHost` produces the same wire classification as the
/// production store.
fn fake_task_graph_conflict(message: String) -> CoreError {
    CoreError::Store(format!("conflict: {message}"))
}
```

In the same file, change `create_agent_chat_task_graph`'s duplicate-id arm from `CoreError::Invariant` to the conflict helper:

```rust
        if self
            .agent_chat_task_graphs
            .lock()
            .expect("agent_chat_task_graphs lock")
            .contains_key(&id)
        {
            return Err(fake_task_graph_conflict(format!(
                "task graph already exists: {id}"
            )));
        }
```

and add the two guards to `update_agent_chat_task_graph_node`, immediately after each lookup:

```rust
        let Some(graph) = graphs.get_mut(graph_id) else {
            return Ok(None);
        };
        if graph.status != "active" {
            return Err(fake_task_graph_conflict(format!(
                "task graph '{graph_id}' is {} and no longer accepts node updates",
                graph.status
            )));
        }
        let Some(node) = graph.nodes.get_mut(node_id) else {
            return Ok(None);
        };
        if fake_task_graph_node_terminal(&node.status) {
            return Err(fake_task_graph_conflict(format!(
                "task graph node '{node_id}' is already {}",
                node.status
            )));
        }
        fake_apply_task_graph_node_patch(node, input)?;
```

- [ ] **Step 8: Run the HTTP tests to verify they pass**

Run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS. Note that `http_agent_chat_task_graph_crud_dispatch_and_node_updates` still passes — it patches node `a` once while the graph is active and only then deletes.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS — `daemon_router_agent_chat_task_graphs_persist_after_router_rebuild` patches each node at most once.

- [ ] **Step 9: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-surface --all-targets -- -D warnings
git add crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs \
        crates/agentd-surface/src/http.rs \
        crates/agentd-surface/src/test_support.rs \
        crates/agentd-surface/tests/http.rs
git commit -m "feat(task-graph): settled nodes are immutable and conflicts return 409"
```

---

### Task 2: Migration 0026 — versioned task-graph writes

The whole graph is one `raw_json` blob written by a read-modify-write over the pool. Two sibling nodes completing concurrently — the fan-out case the feature exists for — reliably drop one result. Add a `record_version` column and make every write a compare-and-set with a `rows_affected` guard, with a bounded retry at each mutation entry point.

**Why not a `BEGIN IMMEDIATE` region.** The advance path calls `agent_scheduler_repo::dispatch`, `agent_scheduler_repo::release` and `message_repo::insert_direct_message`, all of which take `&SqlitePool` and open their own connections. Threading a single `&mut SqliteConnection` through them is a cross-repo refactor far larger than this task. The versioned `UPDATE … WHERE record_version = ?` with a `rows_affected == 1` guard gives the same lost-update safety — that guard is the constraint's intent — and the retry loop makes the operation a correct compare-and-swap. Retries are safe because every side effect the advance can emit is idempotent: dispatch messages use the deterministic id `msg_task_graph_dispatch_<graph>_<node>` inserted `ON CONFLICT(id) DO NOTHING`.

**Files:**
- Create: `crates/agentd-store/migrations/0026_task_graph_record_version.sql`
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs` (`AgentChatTaskGraphRecord`, `upsert_graph`, `row_to_graph`, `graph_select_sql`, `create_graph`, `advance_graph`, `update_node_and_advance`, `delete_graph`, `handle_result_message`)
- Modify: `crates/agentd-store/tests/migration.rs` (every `assert_eq!(version, "25")` → `"26"`)
- Modify: `crates/agentd-store/tests/operational_doctor.rs` (`report.schema_version, 25` → `26`)
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs`

**Interfaces:**
- Consumes: Task 1's `StoreError::Conflict` convention.
- Produces: `AgentChatTaskGraphRecord.record_version: u64` (column-backed, `#[serde(default, skip_serializing)]` so the wire shape is unchanged); `async fn upsert_graph(pool: &SqlitePool, graph: &mut AgentChatTaskGraphRecord) -> Result<(), StoreError>` (note `&mut`); `const MAX_GRAPH_WRITE_ATTEMPTS: usize = 8;` and `fn is_concurrent_write_conflict(error: &StoreError) -> bool`. Tasks 3–5 call `upsert_graph` with `&mut`.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
#[tokio::test]
async fn concurrent_sibling_node_completions_all_land() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    for index in 0..8 {
        nodes.insert(format!("n{index}"), node("codex-a", "Do work", &[]));
    }
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_concurrent".to_string()),
            owner: "orchestrator".to_string(),
            label: "Concurrent graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    let mut handles = Vec::new();
    for index in 0..8 {
        let pool = store.pool().clone();
        handles.push(tokio::spawn(async move {
            agent_chat_task_graph_repo::update_node_and_advance(
                &pool,
                "graph_concurrent",
                &format!("n{index}"),
                agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
                    status: Some("complete".to_string()),
                    result: Some(json!({"node": index})),
                    error: None,
                },
            )
            .await
        }));
    }
    for handle in handles {
        handle
            .await
            .expect("join")
            .expect("concurrent completion succeeds")
            .expect("graph and node");
    }

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_concurrent")
        .await
        .expect("read graph")
        .expect("graph present");
    for index in 0..8 {
        let node = &graph.nodes[&format!("n{index}")];
        assert_eq!(node.status, "complete", "node n{index} lost its completion");
        assert_eq!(node.result, Some(json!({"node": index})));
    }
    assert_eq!(graph.status, "complete");
}

#[tokio::test]
async fn every_graph_write_bumps_the_record_version() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_versioned".to_string()),
            owner: "orchestrator".to_string(),
            label: "Versioned graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    let created_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert_eq!(created_version, 1);

    agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        "graph_versioned",
        "a",
        agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some("complete".to_string()),
            result: Some(json!({"ok": true})),
            error: None,
        },
    )
    .await
    .expect("complete node a")
    .expect("graph and node");

    let updated_version = scalar_count(
        &store,
        "SELECT record_version FROM agent_chat_task_graphs WHERE id = 'graph_versioned'",
    )
    .await;
    assert!(
        updated_version > created_version,
        "record_version must advance on every write: {created_version} -> {updated_version}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs record_version`
Expected: FAIL with `no such column: record_version`.

Then run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs concurrent_sibling`
Expected: FAIL — at least one node is still `dispatched` and the graph is still `active` (the blind overwrite dropped it).

- [ ] **Step 3: Add the migration**

Create `crates/agentd-store/migrations/0026_task_graph_record_version.sql`:

```sql
-- M3 Plan C: optimistic concurrency for the live task-graph row. The whole
-- graph is one `raw_json` blob, so two node results committed at the same
-- moment previously blind-overwrote each other. Every write now carries
-- `WHERE record_version = ?` with a rows_affected guard; the advance path
-- cannot run inside one BEGIN IMMEDIATE because it calls pool-based scheduler
-- and message repos, so the version predicate is what serializes it.
ALTER TABLE agent_chat_task_graphs ADD COLUMN record_version INTEGER NOT NULL DEFAULT 1;

UPDATE schema_meta SET value = '26' WHERE key = 'version';
```

- [ ] **Step 4: Sweep the schema-version assertions**

In `crates/agentd-store/tests/migration.rs`, change every `assert_eq!(version, "25");` to `assert_eq!(version, "26");`.
In `crates/agentd-store/tests/operational_doctor.rs`, change `assert_eq!(report.schema_version, 25);` to `assert_eq!(report.schema_version, 26);`.

Run: `cargo nextest run -p agentd-store --test migration`
Expected: PASS.

Then run: `cargo nextest run -p agentd-store --test operational_doctor`
Expected: PASS.

- [ ] **Step 5: Thread the version through the repo**

In `crates/agentd-store/src/agent_chat_task_graph_repo.rs`:

Add the field to the record (after `completed_at`):

```rust
    #[serde(
        rename = "completedAt",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub completed_at: Option<String>,
    /// Column-backed optimistic-concurrency version. Never serialized into
    /// `raw_json` (the column is the authority) and never deserialized from it;
    /// `0` means "not yet inserted".
    #[serde(default, skip_serializing)]
    pub record_version: u64,
```

Add the retry helpers next to `node_terminal`:

```rust
const MAX_GRAPH_WRITE_ATTEMPTS: usize = 8;
const CONCURRENT_WRITE_SUFFIX: &str = "changed concurrently";

fn concurrent_write_conflict(graph_id: &str) -> StoreError {
    StoreError::Conflict(format!("task graph '{graph_id}' {CONCURRENT_WRITE_SUFFIX}"))
}

fn is_concurrent_write_conflict(error: &StoreError) -> bool {
    matches!(error, StoreError::Conflict(message) if message.ends_with(CONCURRENT_WRITE_SUFFIX))
}
```

Replace `upsert_graph`, `graph_select_sql` and `row_to_graph`:

```rust
async fn upsert_graph(
    pool: &SqlitePool,
    graph: &mut AgentChatTaskGraphRecord,
) -> Result<(), StoreError> {
    validate_member("graph status", &graph.status, GRAPH_STATUSES)?;
    validate_graph_nodes(&graph.nodes)?;
    let raw_json = serde_json::to_string(graph)?;
    let now = now_unix();
    if graph.record_version == 0 {
        sqlx::query(
            "INSERT INTO agent_chat_task_graphs \
             (id, owner, label, status, raw_json, record_version, imported_at) \
             VALUES (?, ?, ?, ?, ?, 1, ?)",
        )
        .bind(&graph.id)
        .bind(&graph.owner)
        .bind(&graph.label)
        .bind(&graph.status)
        .bind(raw_json)
        .bind(now)
        .execute(pool)
        .await?;
        graph.record_version = 1;
        return Ok(());
    }
    let next_version = graph.record_version.saturating_add(1);
    let updated = sqlx::query(
        "UPDATE agent_chat_task_graphs SET owner = ?, label = ?, status = ?, raw_json = ?, \
         record_version = ?, imported_at = ? WHERE id = ? AND record_version = ?",
    )
    .bind(&graph.owner)
    .bind(&graph.label)
    .bind(&graph.status)
    .bind(raw_json)
    .bind(i64::try_from(next_version).unwrap_or(i64::MAX))
    .bind(now)
    .bind(&graph.id)
    .bind(i64::try_from(graph.record_version).unwrap_or(i64::MAX))
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(concurrent_write_conflict(&graph.id));
    }
    graph.record_version = next_version;
    Ok(())
}

fn graph_select_sql(tail: &str) -> String {
    format!(
        "SELECT id, owner, label, status, raw_json, record_version \
         FROM agent_chat_task_graphs {tail}"
    )
}
```

and in `row_to_graph`, set the version from the column just before `Ok(graph)`:

```rust
    graph.record_version = u64::try_from(row.get::<i64, _>("record_version")).unwrap_or(1);
    Ok(graph)
```

- [ ] **Step 6: Make every mutation entry point a bounded compare-and-swap**

Still in `agent_chat_task_graph_repo.rs`, `create_graph` builds an owned record — make it `mut` and pass `&mut`:

```rust
    let mut graph = AgentChatTaskGraphRecord {
        id,
        owner,
        label,
        status: "active".to_string(),
        nodes,
        created_at: now.clone(),
        updated_at: now,
        completed_at: None,
        record_version: 0,
    };
    upsert_graph(pool, &mut graph).await?;
    Ok(graph)
```

`advance_graph_record` takes `mut graph` already; change its two `upsert_graph(pool, &graph)` calls to `upsert_graph(pool, &mut graph)`. Same for `delete_graph`'s single call and `dispatch_drained_task_graph_ticket`'s single call.

Wrap the three public mutation entry points in the retry loop. `advance_graph`:

```rust
pub async fn advance_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        let Some(graph) = get_graph(pool, id).await? else {
            return Ok(None);
        };
        match advance_graph_record(pool, graph).await {
            Ok(graph) => return Ok(Some(graph)),
            Err(error) if is_concurrent_write_conflict(&error) => continue,
            Err(error) => return Err(error),
        }
    }
    Err(concurrent_write_conflict(id))
}
```

`update_node_and_advance` — rename the existing body (with Task 1's guards) to `update_node_and_advance_once` and wrap it:

```rust
pub async fn update_node_and_advance(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    patch: UpdateAgentChatTaskGraphNode,
) -> Result<Option<(AgentChatTaskGraphRecord, AgentChatTaskGraphNode)>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        match update_node_and_advance_once(pool, graph_id, node_id, patch.clone()).await {
            Err(error) if is_concurrent_write_conflict(&error) => continue,
            other => return other,
        }
    }
    Err(concurrent_write_conflict(graph_id))
}
```

`delete_graph` — same shape:

```rust
pub async fn delete_graph(
    pool: &SqlitePool,
    id: &str,
) -> Result<Option<AgentChatTaskGraphRecord>, StoreError> {
    for _ in 0..MAX_GRAPH_WRITE_ATTEMPTS {
        match delete_graph_once(pool, id).await {
            Err(error) if is_concurrent_write_conflict(&error) => continue,
            other => return other,
        }
    }
    Err(concurrent_write_conflict(id))
}
```

where `delete_graph_once` is the existing body verbatim.

`UpdateAgentChatTaskGraphNode` already derives `Clone`, so `patch.clone()` compiles.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS, including `concurrent_sibling_node_completions_all_land` and `every_graph_write_bumps_the_record_version`.

Then run: `cargo nextest run -p agentd-store --test agent_chat_import`
Expected: PASS — `upsert_imported_task_graph` writes the column-defaulted `record_version = 1`.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
git add crates/agentd-store/migrations/0026_task_graph_record_version.sql \
        crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs \
        crates/agentd-store/tests/migration.rs \
        crates/agentd-store/tests/operational_doctor.rs
git commit -m "feat(task-graph): versioned graph writes so concurrent node results cannot be lost"
```

---

### Task 3: Migration 0027 — a graph node enters the M2 durable queue

Give a node an optional `execution` spec. A node that has one is not messaged to an assignee: it gets a `task_runs` row carrying the spec and is enqueued into `execution_task_queue`, so an online native worker pulls and runs it — the same path M2 Plan B's `dispatch_task_to_fleet` proved. A node without an `execution` keeps the p227 message behaviour unchanged.

**Files:**
- Create: `crates/agentd-store/migrations/0027_task_graph_node_executions.sql`
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs`
- Modify: `crates/agentd-surface/src/host.rs` (mirror the two new node fields)
- Modify: `crates/agentd-bin/src/host.rs` (map the two new fields both ways)
- Modify: `crates/agentd-surface/src/test_support.rs` (`FakeRunHost` node construction gains the two fields)
- Modify: `crates/agentd-store/tests/migration.rs`, `crates/agentd-store/tests/operational_doctor.rs` (26 → 27)
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs`

**Interfaces:**
- Consumes: Task 2's `upsert_graph(pool, &mut graph)` signature and retry helpers.
- Produces: `AgentChatTaskGraphNode.execution: Option<Value>` (wire name `execution`) and `AgentChatTaskGraphNode.execution_task_id: Option<String>` (wire name `executionTaskId`); `AgentChatTaskGraphNodeInput.execution: Option<Value>`; table `task_graph_node_executions(graph_id, node_id, execution_task_id, run_id, settled, created_at, settled_at)`; the request-id convention `format!("task-graph-{graph_id}-{node_id}")` for the durable-queue enqueue. Task 4 reads the table and the convention.

- [ ] **Step 1: Write the failing test**

First, `AgentChatTaskGraphNodeInput` gains a field, so the file's two existing struct-literal helpers stop compiling. Add `execution: None,` after `condition` in **both** `node()` and `scheduled_node()` in `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
        condition: None,
        execution: None,
    }
}
```

Then append to the same file:

```rust
fn native_node(
    provider: &str,
    program: &str,
    depends_on: &[&str],
) -> agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
    agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
        id: None,
        assignee: None,
        role: None,
        capability: None,
        description: "Run the native step".to_string(),
        depends_on: depends_on
            .iter()
            .map(std::string::ToString::to_string)
            .collect(),
        condition: None,
        execution: Some(json!({
            "version": 1,
            "provider": provider,
            "program": program,
            "args": [],
            "cwd": null,
            "env": []
        })),
    }
}

#[tokio::test]
async fn a_node_with_an_execution_spec_is_queued_for_a_native_worker() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native".to_string()),
            owner: "orchestrator".to_string(),
            label: "Native graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    // `create_graph` persists; `advance_graph` dispatches.
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native")
        .await
        .expect("advance graph")
        .expect("graph present");

    let node = &graph.nodes["build"];
    assert_eq!(node.status, "dispatched");
    let execution_task_id = node
        .execution_task_id
        .clone()
        .expect("dispatched native node records its execution task id");
    assert_eq!(node.message_id, None, "native nodes are not messaged");
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM direct_messages").await,
        0
    );

    let (queue_status, provider): (String, Option<String>) = sqlx::query_as(
        "SELECT q.status, json_extract(t.execution_spec_json, '$.provider') \
         FROM execution_task_queue q JOIN task_runs t ON t.id = q.execution_task_id \
         WHERE q.execution_task_id = ?",
    )
    .bind(&execution_task_id)
    .fetch_one(store.pool())
    .await
    .expect("queue row");
    assert_eq!(queue_status, "queued");
    assert_eq!(provider.as_deref(), Some("codex"));

    assert_eq!(
        scalar_count(
            &store,
            "SELECT COUNT(*) FROM task_graph_node_executions \
             WHERE graph_id = 'graph_native' AND node_id = 'build' AND settled = 0"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn re_advancing_a_native_node_does_not_enqueue_it_twice() {
    let (store, _dir) = open_store().await;
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native_replay".to_string()),
            owner: "orchestrator".to_string(),
            label: "Native replay graph".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");

    for _ in 0..3 {
        agent_chat_task_graph_repo::advance_graph(store.pool(), "graph_native_replay")
            .await
            .expect("advance")
            .expect("graph present");
    }

    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM execution_task_queue").await,
        1
    );
    assert_eq!(
        scalar_count(&store, "SELECT COUNT(*) FROM task_graph_node_executions").await,
        1
    );
}

#[tokio::test]
async fn a_node_cannot_be_both_scheduler_routed_and_natively_executed() {
    let (store, _dir) = open_store().await;
    let mut conflicting = native_node("codex", "/usr/bin/codex", &[]);
    conflicting.role = Some("coding".to_string());
    let mut nodes = BTreeMap::new();
    nodes.insert("build".to_string(), conflicting);

    let error = agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_native_conflict".to_string()),
            owner: "orchestrator".to_string(),
            label: "Conflicting graph".to_string(),
            nodes,
        },
    )
    .await
    .expect_err("role and execution are mutually exclusive");
    assert!(
        matches!(&error, agentd_store::StoreError::Invariant(message)
            if message.contains("role") && message.contains("execution")),
        "expected an invariant naming both fields, got: {error}"
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs native`
Expected: FAIL to compile — `AgentChatTaskGraphNodeInput` has no field `execution`.

- [ ] **Step 3: Add the migration**

Create `crates/agentd-store/migrations/0027_task_graph_node_executions.sql`:

```sql
-- M3 Plan C: the durable link between a task-graph node and the M2 durable
-- scheduler queue row that executes it. Before this, graph dispatch could only
-- send a direct message and wait for a `task_graph_result` reply; a node with
-- an `execution` spec now gets a `task_runs` row and an `execution_task_queue`
-- entry, and the daemon settles the node from that row's terminal status.
-- The (graph_id, node_id) primary key is what makes re-advancing a node
-- idempotent after a crash between task creation and the graph write.
CREATE TABLE task_graph_node_executions (
    graph_id          TEXT NOT NULL CHECK (length(trim(graph_id)) > 0),
    node_id           TEXT NOT NULL CHECK (length(trim(node_id)) > 0),
    execution_task_id TEXT NOT NULL UNIQUE REFERENCES task_runs(id),
    run_id            TEXT NOT NULL,
    settled           INTEGER NOT NULL DEFAULT 0 CHECK (settled IN (0, 1)),
    created_at        INTEGER NOT NULL,
    settled_at        INTEGER,
    PRIMARY KEY (graph_id, node_id)
);

CREATE INDEX idx_task_graph_node_executions_open
    ON task_graph_node_executions(settled, execution_task_id);

UPDATE schema_meta SET value = '27' WHERE key = 'version';
```

- [ ] **Step 4: Sweep the schema-version assertions**

In `crates/agentd-store/tests/migration.rs`, change every `assert_eq!(version, "26");` to `assert_eq!(version, "27");`.
In `crates/agentd-store/tests/operational_doctor.rs`, change `assert_eq!(report.schema_version, 26);` to `assert_eq!(report.schema_version, 27);`.

Run: `cargo nextest run -p agentd-store --test migration`
Expected: PASS.

Then run: `cargo nextest run -p agentd-store --test operational_doctor`
Expected: PASS.

- [ ] **Step 5: Add the node fields and the native dispatch branch**

In `crates/agentd-store/src/agent_chat_task_graph_repo.rs`, add to `AgentChatTaskGraphNode` (after `condition`):

```rust
    /// A `NativeExecutionSpec`-shaped object. When present the node is executed
    /// by a native worker through the M2 durable queue instead of being
    /// messaged to an assignee.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Value>,
    #[serde(
        default,
        rename = "executionTaskId",
        alias = "execution_task_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub execution_task_id: Option<String>,
```

and to `AgentChatTaskGraphNodeInput` (after `condition`):

```rust
    #[serde(default)]
    pub execution: Option<Value>,
```

In `create_graph`'s node construction, carry them through:

```rust
                condition: input_node.condition,
                execution: input_node.execution,
                execution_task_id: None,
                message_id: None,
```

Relax and extend `validate_graph_nodes`' per-node checks — replace the assignee-or-role block with:

```rust
        if node.role.is_some() && node.execution.is_some() {
            return Err(StoreError::Invariant(format!(
                "node '{id}' cannot set both role and execution"
            )));
        }
        if clean_text(Some(node.assignee.clone())).is_none()
            && node.role.as_deref().and_then(clean_str).is_none()
            && node.execution.is_none()
        {
            return Err(StoreError::Invariant(format!(
                "node '{id}' assignee, role, or execution required"
            )));
        }
        if let Some(execution) = node.execution.as_ref() {
            parse_execution_spec(id, execution)?;
        }
```

Add the spec parser and the dispatch primitive near `dispatch_scheduled_node`:

```rust
use agentd_core::types::{NativeExecutionSpec, NodeId, RunId};

fn parse_execution_spec(
    node_id: &str,
    execution: &Value,
) -> Result<NativeExecutionSpec, StoreError> {
    let spec: NativeExecutionSpec = serde_json::from_value(execution.clone())
        .map_err(|error| StoreError::Invariant(format!("node '{node_id}' execution: {error}")))?;
    spec.validate()
        .map_err(|message| StoreError::Invariant(format!("node '{node_id}' execution: {message}")))?;
    Ok(spec)
}

/// Permanent idempotency key for a node's durable-queue enqueue. Deriving it
/// from the graph and node ids (never from a fresh ULID) is what makes a
/// replayed advance return the existing queue row instead of a second one.
fn node_execution_request_id(graph_id: &str, node_id: &str) -> String {
    format!("task-graph-{graph_id}-{node_id}")
}

async fn existing_node_execution(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
) -> Result<Option<String>, StoreError> {
    let existing: Option<String> = sqlx::query_scalar(
        "SELECT execution_task_id FROM task_graph_node_executions \
         WHERE graph_id = ? AND node_id = ?",
    )
    .bind(graph_id)
    .bind(node_id)
    .fetch_optional(pool)
    .await?;
    Ok(existing)
}

/// Create (or reuse) the node's execution task and enqueue it for a native
/// worker. Ordering is deliberate: the link row is written before the enqueue
/// so a crash can at worst leak an un-enqueued `task_runs` row, never a second
/// queue entry for the same node.
async fn dispatch_native_node(
    pool: &SqlitePool,
    graph_id: &str,
    node: &AgentChatTaskGraphNode,
) -> Result<String, StoreError> {
    if let Some(execution_task_id) = existing_node_execution(pool, graph_id, &node.id).await? {
        return Ok(execution_task_id);
    }
    let execution = node
        .execution
        .as_ref()
        .ok_or_else(|| StoreError::Invariant(format!("node '{}' has no execution", node.id)))?;
    let spec = parse_execution_spec(&node.id, execution)?;
    let run_id = RunId::new();
    crate::run_repo::insert_run(pool, &run_id, &format!("task-graph:{graph_id}")).await?;
    let task_id = crate::task_repo::insert_task_run_with_spec(
        pool,
        &run_id,
        &NodeId::parsed(node.id.clone()),
        &spec,
    )
    .await?;
    sqlx::query(
        "INSERT INTO task_graph_node_executions \
         (graph_id, node_id, execution_task_id, run_id, settled, created_at) \
         VALUES (?, ?, ?, ?, 0, ?) ON CONFLICT(graph_id, node_id) DO NOTHING",
    )
    .bind(graph_id)
    .bind(&node.id)
    .bind(task_id.as_str())
    .bind(run_id.as_str())
    .bind(now_unix())
    .execute(pool)
    .await?;
    // A concurrent advance may have won the insert; the winner's task id is
    // the one that gets enqueued.
    let execution_task_id = existing_node_execution(pool, graph_id, &node.id)
        .await?
        .unwrap_or_else(|| task_id.as_str().to_string());
    let observed_at = now_unix();
    agentd_core::ports::DurableSchedulerPort::enqueue(
        &crate::durable_scheduler::SqliteDurableScheduler::new(pool.clone()),
        &agentd_core::ports::SchedulerEnqueueRequest {
            request_id: node_execution_request_id(graph_id, &node.id),
            execution_task_id: agentd_core::types::TaskRunId::from_string(
                execution_task_id.clone(),
            ),
            max_attempts: 3,
            available_at: observed_at,
            enqueued_at: observed_at,
        },
    )
    .await
    .map_err(|error| StoreError::Invariant(format!("enqueue node '{}': {error}", node.id)))?;
    Ok(execution_task_id)
}
```

In `advance_graph_record`, add the native branch **before** the `role` branch so a native node never reaches the reservation pool:

```rust
            if snapshot.execution.is_some() {
                let execution_task_id = dispatch_native_node(pool, &graph.id, &snapshot).await?;
                let dispatched_at = now_text();
                if let Some(node) = graph.nodes.get_mut(&node_id) {
                    node.status = "dispatched".to_string();
                    node.execution_task_id = Some(execution_task_id);
                    node.dispatched_at.get_or_insert(dispatched_at);
                }
            } else if snapshot.role.as_deref().and_then(clean_str).is_some() {
```

(the existing `role` block body is unchanged; the previous `} else {` message branch stays as the final arm.)

- [ ] **Step 6: Mirror the two fields across the surface boundary**

In `crates/agentd-surface/src/host.rs`, add to `AgentChatTaskGraphNode` (after `condition`):

```rust
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Value>,
    #[serde(
        default,
        rename = "executionTaskId",
        alias = "execution_task_id",
        skip_serializing_if = "Option::is_none"
    )]
    pub execution_task_id: Option<String>,
```

and to `AgentChatTaskGraphNodeInput` (after `condition`):

```rust
    #[serde(default)]
    pub execution: Option<Value>,
```

In `crates/agentd-bin/src/host.rs` there are exactly two sites. `surface_agent_chat_task_graph_node` (store → surface, around line 2881) maps every node field; add both new ones after `condition`:

```rust
        condition: node.condition,
        execution: node.execution,
        execution_task_id: node.execution_task_id,
        message_id: node.message_id,
```

`create_agent_chat_task_graph` (surface → store, around line 2323) builds each `agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput`; add the input field after `condition`:

```rust
                                condition: node.condition,
                                execution: node.execution,
```

In `crates/agentd-surface/src/test_support.rs`, the fake's node construction (around line 1606, the site that sets `condition: node_input.condition`) needs both fields so `FakeRunHost` still compiles:

```rust
                    condition: node_input.condition,
                    execution: node_input.execution,
                    execution_task_id: None,
                    message_id: None,
```

The fake does **not** implement native dispatch — it is only used by surface HTTP tests, which never exercise it.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS, including the three new native tests.

Then run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-surface -p agentd-bin --all-targets -- -D warnings
git add crates/agentd-store/migrations/0027_task_graph_node_executions.sql \
        crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs \
        crates/agentd-store/tests/migration.rs \
        crates/agentd-store/tests/operational_doctor.rs \
        crates/agentd-surface/src/host.rs \
        crates/agentd-surface/src/test_support.rs \
        crates/agentd-bin/src/host.rs
git commit -m "feat(task-graph): dispatch execution-bearing nodes through the durable scheduler queue"
```

---

### Task 4: Durable-queue outcomes settle the node and unlock downstream work

Task 3 sends a node into the queue; nothing brings it back. Add a settlement pass that reads terminal `execution_task_queue` statuses through the link table, patches the node, and lets `advance_graph_record` unlock its dependents — then run it on the daemon's existing maintenance tick, right after `scheduler.reconcile`.

**Files:**
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs` (`settle_node_executions`, `delete_graph_once`)
- Modify: `crates/agentd-bin/src/daemon.rs` (`worker_fleet_tick`)
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs`

**Interfaces:**
- Consumes: Task 3's `task_graph_node_executions` table; Task 1's settled-node `Conflict`; Task 2's retry helpers.
- Produces: `pub async fn settle_node_executions(pool: &SqlitePool, observed_at: i64) -> Result<u64, StoreError>` returning the number of link rows settled. Task 6's e2e calls it directly.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
async fn native_pair_graph(store: &SqliteStore, graph_id: &str) -> String {
    let mut nodes = BTreeMap::new();
    nodes.insert(
        "build".to_string(),
        native_node("codex", "/usr/bin/codex", &[]),
    );
    nodes.insert("report".to_string(), node("codex-b", "Report", &["build"]));
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.to_string()),
            owner: "orchestrator".to_string(),
            label: "Native pair".to_string(),
            nodes,
        },
    )
    .await
    .expect("create graph");
    let graph = agent_chat_task_graph_repo::advance_graph(store.pool(), graph_id)
        .await
        .expect("advance graph")
        .expect("graph present");
    graph.nodes["build"]
        .execution_task_id
        .clone()
        .expect("execution task id")
}

async fn force_queue_status(store: &SqliteStore, execution_task_id: &str, status: &str) {
    sqlx::query(
        "UPDATE execution_task_queue SET status = ?, current_lease_id = NULL, \
         last_reason = 'test forced', updated_at = 0 WHERE execution_task_id = ?",
    )
    .bind(status)
    .bind(execution_task_id)
    .execute(store.pool())
    .await
    .expect("force queue status");
}

#[tokio::test]
async fn a_completed_queue_row_completes_its_node_and_unlocks_downstream() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_ok").await;
    force_queue_status(&store, &execution_task_id, "completed").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_ok")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "complete");
    assert_eq!(
        graph.nodes["build"].result.as_ref().and_then(|result| result
            .get("executionTaskId")
            .and_then(serde_json::Value::as_str)),
        Some(execution_task_id.as_str())
    );
    assert_eq!(
        graph.nodes["report"].status, "dispatched",
        "the downstream node must be unlocked by the native completion"
    );

    let again = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 600)
        .await
        .expect("settle again");
    assert_eq!(again, 0, "settlement is idempotent");
}

#[tokio::test]
async fn a_dead_lettered_queue_row_fails_its_node_and_its_graph() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_fail").await;
    force_queue_status(&store, &execution_task_id, "dead_letter").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 1);

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_fail")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "failed");
    assert!(
        graph.nodes["build"]
            .error
            .as_deref()
            .is_some_and(|error| error.contains("test forced")),
        "the queue reason must reach the node: {:?}",
        graph.nodes["build"].error
    );
    assert_eq!(graph.nodes["report"].status, "failed");
    assert_eq!(graph.status, "failed");
}

#[tokio::test]
async fn deleting_a_graph_settles_its_open_node_executions() {
    let (store, _dir) = open_store().await;
    let execution_task_id = native_pair_graph(&store, "graph_settle_deleted").await;

    agent_chat_task_graph_repo::delete_graph(store.pool(), "graph_settle_deleted")
        .await
        .expect("delete graph")
        .expect("graph present");
    force_queue_status(&store, &execution_task_id, "completed").await;

    let settled = agent_chat_task_graph_repo::settle_node_executions(store.pool(), 500)
        .await
        .expect("settle");
    assert_eq!(settled, 0, "a deleted graph has no open node executions");

    let graph = agent_chat_task_graph_repo::get_graph(store.pool(), "graph_settle_deleted")
        .await
        .expect("read graph")
        .expect("graph present");
    assert_eq!(graph.nodes["build"].status, "cancelled");
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs settle`
Expected: FAIL to compile — `settle_node_executions` does not exist.

- [ ] **Step 3: Implement settlement**

In `crates/agentd-store/src/agent_chat_task_graph_repo.rs`, add:

```rust
/// `(graph_id, node_id, execution_task_id, queue_status, last_reason)`
type NodeSettlementRow = (String, String, String, String, Option<String>);

/// Drive task-graph nodes from the durable scheduler's terminal queue statuses.
/// Called every daemon maintenance tick, immediately after
/// `SqliteDurableScheduler::reconcile` has mapped terminal leases onto queue
/// rows. Returns the number of link rows settled.
///
/// # Errors
/// [`StoreError`] if the link/queue query or a node update fails.
pub async fn settle_node_executions(
    pool: &SqlitePool,
    observed_at: i64,
) -> Result<u64, StoreError> {
    let rows: Vec<NodeSettlementRow> = sqlx::query_as(
        "SELECT e.graph_id, e.node_id, e.execution_task_id, q.status, q.last_reason \
         FROM task_graph_node_executions e \
         JOIN execution_task_queue q ON q.execution_task_id = e.execution_task_id \
         WHERE e.settled = 0 AND q.status IN ('completed', 'dead_letter', 'cancelled')",
    )
    .fetch_all(pool)
    .await?;
    let mut settled = 0_u64;
    for (graph_id, node_id, execution_task_id, queue_status, last_reason) in rows {
        let patch = match queue_status.as_str() {
            "completed" => UpdateAgentChatTaskGraphNode {
                status: Some("complete".to_string()),
                result: Some(json!({
                    "executionTaskId": execution_task_id,
                    "queueStatus": queue_status,
                })),
                error: None,
            },
            "cancelled" => UpdateAgentChatTaskGraphNode {
                status: Some("cancelled".to_string()),
                result: None,
                error: None,
            },
            _ => UpdateAgentChatTaskGraphNode {
                status: Some("failed".to_string()),
                result: None,
                error: Some(
                    last_reason.unwrap_or_else(|| "execution dead-lettered".to_string()),
                ),
            },
        };
        match update_node_and_advance(pool, &graph_id, &node_id, patch).await {
            Ok(_) => {}
            // The node or its graph settled by another route (an operator
            // cancelled the graph, a late result message landed). The
            // execution is finished either way, so close the link row.
            Err(error) if matches!(error, StoreError::Conflict(_)) => {}
            Err(error) => return Err(error),
        }
        mark_node_execution_settled(pool, &graph_id, &node_id, observed_at).await?;
        settled += 1;
    }
    Ok(settled)
}

async fn mark_node_execution_settled(
    pool: &SqlitePool,
    graph_id: &str,
    node_id: &str,
    observed_at: i64,
) -> Result<(), StoreError> {
    let updated = sqlx::query(
        "UPDATE task_graph_node_executions SET settled = 1, settled_at = ? \
         WHERE graph_id = ? AND node_id = ? AND settled = 0",
    )
    .bind(observed_at)
    .bind(graph_id)
    .bind(node_id)
    .execute(pool)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "node execution '{graph_id}/{node_id}' was settled concurrently"
        )));
    }
    Ok(())
}
```

Close open links when a graph is deleted — add to `delete_graph_once`, just before `upsert_graph`:

```rust
    // The graph is cancelled, so its nodes must never be rewritten by a later
    // settlement pass. The in-flight queue rows are left to finish and are
    // discarded; M2 exposes no scheduler cancel primitive (see plan non-goals).
    sqlx::query(
        "UPDATE task_graph_node_executions SET settled = 1, settled_at = ? \
         WHERE graph_id = ? AND settled = 0",
    )
    .bind(now_unix())
    .bind(&graph.id)
    .execute(pool)
    .await?;
    upsert_graph(pool, &mut graph).await?;
```

- [ ] **Step 4: Run the store test to verify it passes**

Run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS, including all three settlement tests.

- [ ] **Step 5: Run settlement on the daemon maintenance tick**

In `crates/agentd-bin/src/daemon.rs`, extend `worker_fleet_tick`:

```rust
pub async fn worker_fleet_tick(
    fleet: &dyn WorkerFleetPort,
    recovery_registry: &NativeRecoveryRegistry,
    native_worker: &AgentdWorker,
    scheduler: &agentd_store::durable_scheduler::SqliteDurableScheduler,
    observed_at: i64,
) {
    let _ = fleet.recover_offline(observed_at - 30).await;
    let _ = fleet.expire_due(observed_at).await;
    let _ = scheduler.reconcile(observed_at).await;
    // Reconcile maps terminal leases onto queue rows; settlement then maps
    // terminal queue rows onto task-graph nodes. Order matters: a node must
    // never settle from a queue row reconcile has not yet finalized.
    let _ = agentd_store::agent_chat_task_graph_repo::settle_node_executions(
        native_worker.store().pool(),
        observed_at,
    )
    .await;
    let _ = agent_registry_tick(native_worker.store().pool(), observed_at).await;
    let _ = recovery_registry.recover_one(native_worker).await;
}
```

- [ ] **Step 6: Verify the daemon still builds and its tests pass**

Run: `cargo nextest run -p agentd-bin --test native_dispatch`
Expected: PASS.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-bin --all-targets -- -D warnings
git add crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs \
        crates/agentd-bin/src/daemon.rs
git commit -m "feat(task-graph): settle graph nodes from durable-queue outcomes on the daemon tick"
```

---

### Task 5: Imported agent-chat graphs are readable and runnable

p225 stores agent-chat's `task_graphs.json` verbatim, and every read then demands the full live shape. An imported graph missing `createdAt`/`updatedAt`, or a node missing `status`, is unreadable — and because `list_graphs` collects results with `?`, one such row makes `GET /api/task-graphs` fail for every graph. Normalize at import, and make listing degrade per row.

**Files:**
- Modify: `crates/agentd-store/src/agent_chat_import.rs` (`parse_task_graph`, `ImportTaskGraph`)
- Modify: `crates/agentd-store/src/agent_chat_task_graph_repo.rs` (`list_graphs`)
- Test: `crates/agentd-store/tests/agent_chat_import.rs`
- Test: `crates/agentd-store/tests/agent_chat_task_graphs.rs`

**Interfaces:**
- Consumes: Task 2's `record_version` column (import writes the default `1`).
- Produces: `pub fn normalize_imported_task_graph(id: &str, value: &Value, imported_at: i64) -> Option<Value>` in `agent_chat_import` — returns the live-shaped JSON, or `None` when the graph cannot be normalized (counted as skipped). `list_graphs` skips unreadable rows instead of failing.

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-store/tests/agent_chat_import.rs`:

```rust
#[tokio::test]
async fn imported_agent_chat_task_graphs_are_readable_and_advanceable() {
    let source = tempfile::tempdir().expect("source dir");
    write_file(&source.path().join("data/tasks.json"), "[]");
    write_file(
        &source.path().join("data/task_graphs.json"),
        r#"{
  "graph_legacy": {
    "owner": "alex",
    "label": "Legacy graph",
    "status": "active",
    "nodes": {
      "n1": {"assignee": "codex-importer", "description": "First"},
      "n2": {"assignee": "codex-importer", "description": "Second", "dependsOn": ["n1"]}
    }
  }
}
"#,
    );
    let (store, _target) = open_store().await;

    let report = agent_chat_import::import_tasks_from_agent_chat(
        store.pool(),
        source.path(),
        AgentChatTaskImportOptions {
            mode: AgentChatImportMode::Execute,
        },
    )
    .await
    .expect("task import succeeds");
    assert_eq!(report.task_graphs.imported, 1);

    let graph = agentd_store::agent_chat_task_graph_repo::get_graph(store.pool(), "graph_legacy")
        .await
        .expect("read imported graph")
        .expect("graph present");
    assert_eq!(graph.owner, "alex");
    assert_eq!(graph.nodes["n1"].status, "pending");
    assert_eq!(graph.nodes["n2"].depends_on, vec!["n1".to_string()]);
    assert!(!graph.created_at.is_empty());

    let listed = agentd_store::agent_chat_task_graph_repo::list_graphs(store.pool(), None)
        .await
        .expect("list graphs");
    assert_eq!(listed.len(), 1);

    let advanced = agentd_store::agent_chat_task_graph_repo::advance_graph(
        store.pool(),
        "graph_legacy",
    )
    .await
    .expect("advance imported graph")
    .expect("graph present");
    assert_eq!(advanced.nodes["n1"].status, "dispatched");
    assert_eq!(advanced.nodes["n2"].status, "pending");
}
```

Append to `crates/agentd-store/tests/agent_chat_task_graphs.rs`:

```rust
#[tokio::test]
async fn one_unreadable_legacy_row_does_not_break_the_listing() {
    let (store, _dir) = open_store().await;
    agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some("graph_good".to_string()),
            owner: "orchestrator".to_string(),
            label: "Good graph".to_string(),
            nodes: chain_nodes(),
        },
    )
    .await
    .expect("create graph");
    sqlx::query(
        "INSERT INTO agent_chat_task_graphs \
         (id, owner, label, status, raw_json, record_version, imported_at) \
         VALUES ('graph_broken', 'alex', 'Broken', 'active', '{\"nodes\":', 1, 0)",
    )
    .execute(store.pool())
    .await
    .expect("insert unreadable row");

    let listed = agent_chat_task_graph_repo::list_graphs(store.pool(), None)
        .await
        .expect("listing tolerates one unreadable row");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, "graph_good");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p agentd-store --test agent_chat_import imported_agent_chat_task_graphs`
Expected: FAIL — `get_graph` errors with a serde "missing field `createdAt`" message.

Then run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs unreadable_legacy`
Expected: FAIL — `list_graphs` returns an `Err`.

- [ ] **Step 3: Normalize on import**

In `crates/agentd-store/src/agent_chat_import.rs`, replace `parse_task_graph` and add the normalizer:

```rust
fn parse_task_graph(key: &str, value: &Value) -> Result<Option<ImportTaskGraph>, StoreError> {
    let Some(object) = value.as_object() else {
        return Ok(None);
    };
    let id = string_field(object, "id").unwrap_or_else(|| key.to_string());
    if id.trim().is_empty() {
        return Ok(None);
    }
    // Project into the live `AgentChatTaskGraphRecord` shape rather than
    // storing agent-chat's JSON verbatim: every read of `raw_json` is a typed
    // deserialization, so a graph that keeps agent-chat's optional-field
    // conventions would be permanently unreadable after import.
    let Some(normalized) = normalize_imported_task_graph(&id, value) else {
        return Ok(None);
    };
    Ok(Some(ImportTaskGraph {
        id,
        owner: string_field(object, "owner"),
        label: string_field(object, "label"),
        status: string_field(object, "status"),
        raw_json: serde_json::to_string(&normalized)?,
    }))
}

/// Fill the fields the live task-graph record requires but agent-chat treats as
/// optional. Returns `None` when the graph cannot be made live (no usable
/// nodes), in which case the caller counts it as skipped.
fn normalize_imported_task_graph(id: &str, value: &Value) -> Option<Value> {
    let object = value.as_object()?;
    let text = |key: &str, fallback: &str| -> String {
        object
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map_or_else(|| fallback.to_string(), str::to_string)
    };
    let created_at = text("createdAt", "0");
    let updated_at = text("updatedAt", &created_at);
    let mut nodes = serde_json::Map::new();
    for (node_key, node_value) in object.get("nodes").and_then(Value::as_object)? {
        let Some(node_object) = node_value.as_object() else {
            continue;
        };
        let node_id = node_object
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or(node_key.as_str())
            .to_string();
        let mut node = node_object.clone();
        node.insert("id".to_string(), Value::String(node_id.clone()));
        node.entry("assignee")
            .or_insert_with(|| Value::String(String::new()));
        node.entry("description")
            .or_insert_with(|| Value::String(String::new()));
        node.entry("status")
            .or_insert_with(|| Value::String("pending".to_string()));
        if let Some(depends_on) = node.remove("dependsOn") {
            node.entry("depends_on").or_insert(depends_on);
        }
        node.entry("depends_on")
            .or_insert_with(|| Value::Array(Vec::new()));
        nodes.insert(node_id, Value::Object(node));
    }
    if nodes.is_empty() {
        return None;
    }
    let mut graph = serde_json::Map::new();
    graph.insert("id".to_string(), Value::String(id.to_string()));
    graph.insert("owner".to_string(), Value::String(text("owner", "unknown")));
    graph.insert("label".to_string(), Value::String(text("label", id)));
    graph.insert(
        "status".to_string(),
        Value::String(text("status", "active")),
    );
    graph.insert("createdAt".to_string(), Value::String(created_at));
    graph.insert("updatedAt".to_string(), Value::String(updated_at));
    if let Some(completed_at) = object.get("completedAt").filter(|value| !value.is_null()) {
        graph.insert("completedAt".to_string(), completed_at.clone());
    }
    graph.insert("nodes".to_string(), Value::Object(nodes));
    Some(Value::Object(graph))
}
```

- [ ] **Step 4: Make listing degrade per row**

In `crates/agentd-store/src/agent_chat_task_graph_repo.rs`, replace the body of `list_graphs`:

```rust
pub async fn list_graphs(
    pool: &SqlitePool,
    status: Option<&str>,
) -> Result<Vec<AgentChatTaskGraphRecord>, StoreError> {
    let rows = sqlx::query(graph_select_sql("ORDER BY rowid").as_str())
        .fetch_all(pool)
        .await?;
    let status = status.and_then(|value| clean_text(Some(value.to_string())));
    // A legacy row that predates normalization must not take the whole listing
    // down with it; `get_graph` still surfaces the parse error for that id.
    let graphs = rows
        .iter()
        .filter_map(|row| row_to_graph(row).ok())
        .filter(|graph| {
            status
                .as_deref()
                .is_none_or(|status| graph.status == status)
        })
        .collect();
    Ok(graphs)
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test agent_chat_import`
Expected: PASS, including the existing `agent_chat_task_import_execute_preserves_task_and_graph_snapshots` (its fixture graph already carries every field, and normalization preserves `nodes`).

Then run: `cargo nextest run -p agentd-store --test agent_chat_task_graphs`
Expected: PASS.

Then run: `cargo nextest run -p agentctl --test parity_cli`
Expected: PASS — the parity CLI's import fixture goes through the same normalizer.

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
git add crates/agentd-store/src/agent_chat_import.rs \
        crates/agentd-store/src/agent_chat_task_graph_repo.rs \
        crates/agentd-store/tests/agent_chat_import.rs \
        crates/agentd-store/tests/agent_chat_task_graphs.rs
git commit -m "feat(task-graph): normalize imported agent-chat graphs into the live shape"
```

---

### Task 6: M3 exit-gate end-to-end, and parity evidence

The milestone gate is "a project's agents register, message, and run a task graph with no agent-chat process in the path". Prove it in one test that composes M3 Plan A (registry), M3 Plan B (messaging), and this plan's native node path, with a real worker executing the node — and then record the evidence in the parity map with the contract test extended in the same commit.

**Files:**
- Create: `crates/agentd-bin/tests/m3_coordination_e2e.rs`
- Modify: `docs/parity/agent-chat-capability-map.md` (append to the `task_graph_coordination` and `migration_shadow_cutover` decision cells; **statuses stay `partial`**)
- Modify: `crates/agentctl/tests/parity_cli.rs` (`parity_capability_map_records_p227_live_task_graph_progress`)

**Interfaces:**
- Consumes: everything from Tasks 1–5, plus `agentd_bin::daemon::{build_router, daemon_native_runtime_router, recovery_router, WorkerFleetService}` and `agentd_bin::worker_main::run_worker_once` (M1/M2 Plan B).
- Produces: nothing consumed by later tasks — this is the terminal task.

- [ ] **Step 1: Write the failing end-to-end test**

Create `crates/agentd-bin/tests/m3_coordination_e2e.rs`. The fleet fixture mirrors `crates/agentd-bin/tests/native_dispatch.rs` (`dispatch_fixture`, `serve_dispatch_daemon`, `dispatch_authority_snapshot`); duplication across integration-test binaries is the established convention in this repo.

```rust
//! M3 exit gate: a project's agents register, message each other, and run a
//! task graph whose native node is executed by a real worker — with no
//! agent-chat process and no tmux anywhere in the path.

use std::sync::Arc;

use agentd_bin::{ProductionRunHost, SystemClock, daemon};
use agentd_core::test_support::{FakeBackend, MempalStub, RecordingCommandRunner};
use agentd_core::types::{
    AgentProfileId, AuthorityKey, CertificationPolicyVersionRef, FrozenSpecVersionRef,
    MatrixRoomRef, OfflineRecoveryPolicy, OrganizationRef, ProductWorkflowRef,
    ProjectExecutionSnapshot, ProjectExecutionSnapshotRef, ProjectRef, ProjectRoomBindingRef,
    QuotaPolicyVersionRef, RbacPolicyVersionRef, RepositoryBinding, RepositoryRef, RepositoryRole,
    RequirementRef, RoomBinding, RoomBindingRole, TeamRef, WorkerId, WorkerIncarnationId,
};
use agentd_store::SqliteStore;
use agentd_store::worker_repo::{self, WorkerCreate, WorkerRegistration};
use axum::Router;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

fn workflows_dir() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workflows")
}

async fn send(app: Router, method: &str, path: &str, body: Value) -> (StatusCode, Value) {
    let request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

async fn get(app: Router, path: &str) -> (StatusCode, Value) {
    let request = Request::builder()
        .uri(path)
        .body(Body::empty())
        .expect("request");
    let response = app.oneshot(request).await.expect("response");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    (status, value)
}

/// Mirrors `crates/agentd-bin/tests/native_dispatch.rs::dispatch_authority_snapshot`:
/// a minimal internally-consistent snapshot whose ref matches the runtime
/// session created for the native node, so `pull` can resolve a security scope.
fn authority_snapshot() -> ProjectExecutionSnapshot {
    let authority_key = AuthorityKey::new("specify").expect("authority key");
    let project_ref =
        ProjectRef::new(authority_key.clone(), "project-1", "7").expect("project ref");
    let rbac_ref =
        RbacPolicyVersionRef::new(authority_key.clone(), "rbac-1", "4").expect("rbac ref");
    ProjectExecutionSnapshot {
        snapshot_ref: ProjectExecutionSnapshotRef::new(authority_key.clone(), "spec-1", "v1")
            .expect("snapshot ref"),
        authority_key: authority_key.clone(),
        authority_revision: 9,
        organization_ref: OrganizationRef::new(authority_key.clone(), "org-1", "2")
            .expect("organization ref"),
        team_refs: vec![
            TeamRef::new(authority_key.clone(), "team-runtime", "3").expect("team ref"),
        ],
        project_ref: project_ref.clone(),
        repository_bindings: vec![RepositoryBinding {
            repository_ref: RepositoryRef::new(authority_key.clone(), "repo-1", "5")
                .expect("repository ref"),
            role: RepositoryRole::Target,
            forge_locator: Some("github:corp/repo".to_string()),
            base_commit: "0123456789abcdef0123456789abcdef01234567".to_string(),
        }],
        room_bindings: vec![RoomBinding {
            binding_ref: ProjectRoomBindingRef::new(authority_key.clone(), "binding-1", "6")
                .expect("binding ref"),
            project_ref,
            matrix_room_ref: MatrixRoomRef::new(
                AuthorityKey::new("matrix:corp").expect("matrix authority"),
                "!room:corp",
                "11",
            )
            .expect("matrix room ref"),
            roles: vec![RoomBindingRole::Command],
            allowed_command_classes: vec!["execute".to_string()],
            rbac_policy_version_ref: rbac_ref.clone(),
        }],
        issue_ref: None,
        requirement_refs: vec![
            RequirementRef::new(authority_key.clone(), "req-1", "8").expect("requirement ref"),
        ],
        frozen_spec_version_ref: FrozenSpecVersionRef::new(
            authority_key.clone(),
            "spec-doc-1",
            "12",
        )
        .expect("spec ref"),
        product_workflow_ref: ProductWorkflowRef::new(authority_key.clone(), "workflow-1", "13")
            .expect("workflow ref"),
        rbac_policy_version_ref: rbac_ref,
        quota_policy_version_ref: QuotaPolicyVersionRef::new(authority_key.clone(), "quota-1", "14")
            .expect("quota ref"),
        certification_policy_version_ref: Some(
            CertificationPolicyVersionRef::new(authority_key.clone(), "cert-policy-1", "15")
                .expect("certification policy ref"),
        ),
        issued_at: 100,
        valid_until: 4_102_444_800,
        content_sha256: "a".repeat(64),
        offline_recovery_policy: OfflineRecoveryPolicy::Deny,
    }
}

/// Serve the worker-facing fleet/runtime routes on a real socket so a real
/// `agentd worker` loop can pull over HTTP. Mirrors
/// `crates/agentd-bin/tests/native_dispatch.rs::serve_dispatch_daemon`.
async fn serve_fleet(store: SqliteStore, token: &str) -> String {
    let fleet = Arc::new(agentd_store::worker_fleet::SqliteWorkerFleet::new(
        store.pool().clone(),
    ));
    let artifacts = Arc::new(
        agentd_store::content_store::LocalContentStore::new(
            std::env::temp_dir().join(format!("agentd-m3-e2e-artifacts-{}", std::process::id())),
        )
        .expect("content store"),
    );
    let service = Arc::new(daemon::WorkerFleetService::new(
        fleet,
        agentd_bin::native_worker::AgentdWorker::new(store.clone()),
        artifacts,
    ));
    let auth = agentd_surface::http::AuthConfig {
        api_token: Some(token.to_string()),
        ..agentd_surface::http::AuthConfig::default()
    };
    let fleet_router = agentd_surface::worker_fleet_http::worker_fleet_router(
        Arc::new(
            agentd_store::worker_fleet::SqliteWorkerFleet::new(store.pool().clone())
                .with_auth_proof(token.to_string()),
        ),
        auth,
    );
    let app = daemon::daemon_native_runtime_router(&store, Some(token.to_string()))
        .merge(daemon::recovery_router(service, token.to_string()))
        .merge(fleet_router);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn agents_register_message_and_run_a_task_graph_with_no_agent_chat_process() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().expect("tempdir");
    let store = SqliteStore::connect(&dir.path().join("agentd.db"))
        .await
        .expect("connect");
    // `FakeBackend`, not `TmuxBackend`: nothing in this test can launch tmux.
    let host = ProductionRunHost::new(
        store.clone(),
        Box::new(FakeBackend::new()),
        Box::new(RecordingCommandRunner::new()),
        Box::new(MempalStub::new()),
        Box::new(SystemClock),
        workflows_dir(),
    );
    let app = daemon::build_router(Arc::new(host));

    // 1. Register — M3 Plan A. `AgentRegistration` requires only `name`; every
    //    other field is an `Option` and defaults to `None`.
    for name in ["orchestrator", "codex-a"] {
        let (status, body) = send(
            app.clone(),
            "POST",
            "/api/agents",
            json!({
                "name": name,
                "role": "coding",
                "capability": "medium",
                "runtime": "codex",
                "workdir": format!("/tmp/agentd/{name}")
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "register {name}: {body}");
        assert_eq!(body["ok"], true, "register {name}: {body}");
    }
    let (agents_status, agents) = get(app.clone(), "/api/agents").await;
    assert_eq!(agents_status, StatusCode::OK);
    assert!(
        agents.to_string().contains("codex-a"),
        "registry lists the agent: {agents}"
    );

    // 2. Message — M3 Plan B.
    let (message_status, _) = send(
        app.clone(),
        "POST",
        "/api/messages",
        json!({
            "from": "orchestrator",
            "to": "codex-a",
            "summary": "kickoff",
            "full": "starting the graph"
        }),
    )
    .await;
    assert_eq!(message_status, StatusCode::CREATED);
    let (inbox_status, inbox) = get(app.clone(), "/api/inbox/codex-a?drain=false").await;
    assert_eq!(inbox_status, StatusCode::OK);
    assert_eq!(inbox["dm"][0]["summary"], "kickoff");

    // 3. Run a task graph: a messaged node followed by a native node.
    let shim_dir = tempfile::tempdir().expect("shim dir");
    let shim = shim_dir.path().join("codex");
    std::fs::write(&shim, "#!/bin/sh\nexit 0\n").expect("write shim");
    std::fs::set_permissions(&shim, std::fs::Permissions::from_mode(0o755)).expect("chmod");

    let (create_status, created) = send(
        app.clone(),
        "POST",
        "/api/task-graphs",
        json!({
            "id": "graph_m3",
            "owner": "orchestrator",
            "label": "M3 exit gate",
            "nodes": {
                "plan": {"assignee": "codex-a", "description": "Plan the work"},
                "build": {
                    "description": "Build it natively",
                    "depends_on": ["plan"],
                    "execution": {
                        "version": 1,
                        "provider": "codex",
                        "program": shim.to_string_lossy(),
                        "args": [],
                        "cwd": shim_dir.path().to_string_lossy(),
                        "env": []
                    }
                }
            }
        }),
    )
    .await;
    assert_eq!(create_status, StatusCode::OK, "body: {created}");
    assert_eq!(created["graph"]["nodes"]["plan"]["status"], "dispatched");
    assert_eq!(created["graph"]["nodes"]["build"]["status"], "pending");
    let plan_message_id = created["graph"]["nodes"]["plan"]["message_id"]
        .as_str()
        .expect("plan dispatch message id")
        .to_string();

    // The assignee reports the first node's result as a normal message.
    let (result_status, result) = send(
        app.clone(),
        "POST",
        "/api/messages",
        json!({
            "from": "codex-a",
            "to": "orchestrator",
            "summary": "planned",
            "full": "planned",
            "reply_to": plan_message_id,
            "schema": {
                "kind": "task_graph_result",
                "version": 1,
                "payload": {"graphId": "graph_m3", "nodeId": "plan", "result": {"ok": true}}
            }
        }),
    )
    .await;
    assert_eq!(result_status, StatusCode::CREATED, "body: {result}");
    assert_eq!(result["taskGraph"]["status"], "complete");
    let (graph_status, graph) = get(app.clone(), "/api/task-graphs/graph_m3").await;
    assert_eq!(graph_status, StatusCode::OK);
    assert_eq!(graph["nodes"]["build"]["status"], "dispatched");
    let execution_task_id = graph["nodes"]["build"]["executionTaskId"]
        .as_str()
        .expect("native node carries its execution task id")
        .to_string();

    // 4. A real native worker pulls and executes the queued node.
    agentd_store::project_authority_repo::record_snapshot(store.pool(), &authority_snapshot())
        .await
        .expect("record project authority snapshot");
    let profile_id = AgentProfileId::new();
    agentd_store::agent_profile_repo::create_profile(
        store.pool(),
        agentd_store::agent_profile_repo::AgentProfileCreate {
            id: profile_id.clone(),
            role: "implementer".to_string(),
            capability: Some("implementation".to_string()),
            runtime: "codex".to_string(),
            model: Some("gpt-5".to_string()),
            prompt_profile: Some("default".to_string()),
        },
    )
    .await
    .expect("profile");
    let worker_id = WorkerId::new();
    worker_repo::create_worker(
        store.pool(),
        WorkerCreate {
            id: worker_id.clone(),
            trust_domain: "corp-coding".to_string(),
            labels: json!({}),
        },
    )
    .await
    .expect("worker");
    worker_repo::register_incarnation(
        store.pool(),
        &worker_id,
        WorkerRegistration {
            id: WorkerIncarnationId::new(),
            daemon_version: "0.0.0-m3-plan-c".to_string(),
            host_name: "host-a".to_string(),
            network_zone: Some("dev".to_string()),
            capabilities: json!({"runtime": ["codex"]}),
            capacity: 1,
        },
    )
    .await
    .expect("incarnation");
    let session_id = agentd_core::types::RuntimeSessionId::new();
    agentd_store::runtime_session_repo::create_session(
        store.pool(),
        agentd_store::runtime_session_repo::RuntimeSessionCreate {
            id: session_id.clone(),
            execution_task_id: agentd_core::types::TaskRunId::from_string(
                execution_task_id.clone(),
            ),
            agent_profile_id: profile_id,
            snapshot: agentd_store::runtime_session_repo::ExecutionSnapshotRef {
                authority_key: "specify".to_string(),
                resource_kind: "execution_snapshot".to_string(),
                resource_id: "spec-1".to_string(),
                resource_version: "v1".to_string(),
                content_sha256: "a".repeat(64),
            },
        },
    )
    .await
    .expect("session");

    let base_url = serve_fleet(store.clone(), "worker-secret").await;
    let worker_state = tempfile::tempdir().expect("worker state");
    let report = agentd_bin::worker_main::run_worker_once(
        &base_url,
        "worker-secret",
        worker_state.path(),
        std::time::Duration::from_millis(100),
        std::time::Duration::from_secs(30),
    )
    .await
    .expect("worker run");
    assert_eq!(report.executed, 1);
    assert_eq!(report.released, 1);

    // 5. The daemon's maintenance order — reconcile, then settle — closes the
    //    node and therefore the graph.
    let scheduler =
        agentd_store::durable_scheduler::SqliteDurableScheduler::new(store.pool().clone());
    agentd_core::ports::DurableSchedulerPort::reconcile(&scheduler, 1_000)
        .await
        .expect("reconcile");
    let settled = agentd_store::agent_chat_task_graph_repo::settle_node_executions(
        store.pool(),
        1_000,
    )
    .await
    .expect("settle");
    assert_eq!(settled, 1);

    let (final_status, final_graph) = get(app, "/api/task-graphs/graph_m3").await;
    assert_eq!(final_status, StatusCode::OK);
    assert_eq!(final_graph["nodes"]["plan"]["status"], "complete");
    assert_eq!(final_graph["nodes"]["build"]["status"], "complete");
    assert_eq!(final_graph["status"], "complete");
}
```

- [ ] **Step 2: Run the end-to-end test**

Run: `cargo nextest run -p agentd-bin --test m3_coordination_e2e`
Expected: PASS. Two failure modes are worth naming because they are diagnosis, not assertion-weakening: if the worker reports `executed == 0` the queue row was not eligible — confirm the incarnation's `capabilities.runtime` contains `"codex"` (the durable scheduler's capability filter compares it against the spec's `provider`) and that `record_snapshot` ran before the pull, since `SqliteWorkerFleet::pull` needs the snapshot to resolve a security scope. If `settled` is `0`, `reconcile` did not finalize the queue row — check that the worker's release marked the lease `released`. Never relax an assertion to make this test pass.

- [ ] **Step 3: Append the parity evidence**

In `docs/parity/agent-chat-capability-map.md`, append to the **`task_graph_coordination`** decision cell, immediately before the trailing "This remains partial until …" sentence:

> M3 Plan C makes the coordination semantics agentd-owned: settled nodes and non-active graphs reject further writes with a conflict (409), an empty node patch is rejected, migration `0026_task_graph_record_version.sql` adds a `record_version` compare-and-set so concurrent node results cannot overwrite each other, migration `0027_task_graph_node_executions.sql` adds the durable node ↔ `execution_task_queue` link so a node carrying an `execution` spec is dispatched to a native worker through the M2 durable scheduler instead of only being messaged, `settle_node_executions` drives node completion/failure from the queue's terminal status on the daemon maintenance tick, and an end-to-end test registers agents, exchanges messages, and runs a task graph whose native node is executed by a real worker with no agent-chat process and no tmux.

Append to the **`migration_shadow_cutover`** decision cell, immediately before its trailing "This remains partial until …" sentence:

> M3 Plan C makes imported agent-chat task graphs live rather than merely preserved: import normalizes `task_graphs.json` into the live record shape (filling `createdAt`/`updatedAt`, node `status`, and `dependsOn` → `depends_on`) instead of storing the source JSON verbatim, graphs that cannot be normalized are counted as skipped, listing tolerates an unreadable legacy row instead of failing wholesale, and an imported graph can be read, listed, and advanced.

**Do not change either `status` cell — both stay `partial`.**

- [ ] **Step 4: Extend the parity contract test**

In `crates/agentctl/tests/parity_cli.rs`, inside `parity_capability_map_records_p227_live_task_graph_progress`, add the new evidence phrases. Extend the shared loop's expectation list:

```rust
    for row in [task_graph, migration] {
        assert_eq!(row.status, "partial");
        for expected in [
            "p227",
            "live",
            "/api/task-graphs",
            "dispatch",
            "scheduler",
            "dashboard",
            "Matrix",
            "remote relay",
            "service cutover",
            "rollback",
            "token provisioning",
            "M3 Plan C",
        ] {
            assert!(
                row.decision.contains(expected),
                "{} decision should mention {expected}: {}",
                row.capability,
                row.decision
            );
        }
    }

    for expected in [
        "record_version",
        "task_graph_node_executions",
        "settle_node_executions",
    ] {
        assert!(
            task_graph.decision.contains(expected),
            "task_graph_coordination decision should mention {expected}: {}",
            task_graph.decision
        );
    }

    for expected in ["normaliz", "imported"] {
        assert!(
            migration.decision.contains(expected),
            "migration_shadow_cutover decision should mention {expected}: {}",
            migration.decision
        );
    }
```

(the roadmap assertion block below it is unchanged.)

- [ ] **Step 5: Run the contract test**

Run: `cargo nextest run -p agentctl --test parity_cli`
Expected: PASS.

Then run: `cargo nextest run -p agentctl --test worktree_reconciliation_contract`
Expected: PASS (it does not mention these rows).

Then run: `cargo nextest run -p agentctl --test enterprise_project_authority_contract`
Expected: PASS (it does not mention these rows).

- [ ] **Step 6: Commit**

```bash
cargo fmt --all
cargo clippy -p agentd-bin -p agentctl --all-targets -- -D warnings
git add crates/agentd-bin/tests/m3_coordination_e2e.rs \
        docs/parity/agent-chat-capability-map.md \
        crates/agentctl/tests/parity_cli.rs
git commit -m "test(m3): prove the coordination exit gate end to end and record parity evidence"
```

---

## Final gate (after Task 6, before the whole-branch review)

Run the full workspace suite **once**, alone, with no other `nextest` process running:

```bash
cargo nextest run --workspace
```

Expected: all tests pass except the known load-sensitive flake `native_runtime_can_terminate_a_running_child` (`agentd-tmux`, untouched by this branch), which passes in isolation. Re-run it alone to confirm:

```bash
cargo nextest run -p agentd-tmux native_runtime_can_terminate_a_running_child
```

## Release notes for the merge

- **Behaviour change:** `PATCH /api/task-graphs/:id/nodes/:node` now returns **409** when the node is already `complete`/`failed`/`skipped`/`cancelled`, or when the graph is no longer `active`. Clients that re-sent a completion as a retry must treat 409 as success.
- **Behaviour change:** an empty node patch (`{}`) now returns **400**, matching agent-chat's `invalid_patch`.
- **Behaviour change:** `POST /api/task-graphs` with an existing id now returns **409** instead of 500.
- **New:** a task-graph node may carry an `execution` object (a `NativeExecutionSpec`: `version`, `provider`, `program`, `args`, `cwd`, `env`). Such a node is executed by a native worker through the durable scheduler queue and is settled by the daemon; it is **not** messaged to an assignee, and it must not also set `role`.
- **Schema:** version 25 → **27** (`0026_task_graph_record_version.sql`, `0027_task_graph_node_executions.sql`).
- **Known limitation:** deleting a graph with an in-flight native node does not cancel that node's queue row (M2 exposes no scheduler cancel primitive). The execution runs to completion and its outcome is discarded. Follow-up ticket: `DurableSchedulerPort::cancel`.
