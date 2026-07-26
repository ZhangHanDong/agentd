# M3 Plan B — Messaging Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the remaining live-path divergences between agentd's direct/group messaging and agent-chat's, so read cursors advance, mentions are scoped to real group members, structured message schemas survive the MCP send path, and a single message can be dropped for one recipient — with no agent-chat process in the path.

**Architecture:** All four parity gaps are behavioural, not structural: the durable tables (`direct_messages`, `group_messages`, `group_message_reads`, `group_mention_reads`, `group_members`) added by p217/p220 already model everything agent-chat's `cursors.json` modelled, and p224 already imports agent-chat cursor state into them. The work is therefore (a) surface-layer changes in `agentd-surface` (`check_inbox` default advance + `kinds` filter, `send_message` `schema` passthrough), (b) one SQL predicate change in `agentd-store::message_repo` (mentions scoped to current membership), and (c) one new store operation plus its `RunHost`/HTTP vertical (per-recipient suppression). **No task in this plan changes the database schema, so no migration is added and `schema_meta.version` stays at 25.** One task up front lands three hardening carry-overs from the M3 Plan A final review.

**Tech Stack:** Rust 2024, tokio, axum 0.7-style `Router`/extractors, sqlx + SQLite, serde/serde_json, `cargo nextest`, `tempfile`, `tower::ServiceExt::oneshot`, `http_body_util::BodyExt`.

## Global Constraints

- **Error classification:** `Invalid` → 400, `NotFound` → 404, `Conflict` → 409, and only `Unavailable` is retryable → 503. The `ControlPlaneErrorStatus` trait in `agentd-surface` is the mapping pattern.
- **Multi-statement mutations** run inside `BEGIN IMMEDIATE` with `rows_affected` guards on every write.
- **Liveness columns** (`status`, `offline_reason`, `last_seen_at`) are owned only by heartbeat / start / offline / sweep. Nothing in this plan may write them.
- **`agentd-surface` stays store-free.** It depends on `agentd-core` ports and its own `RunHost` trait only — never on `agentd-store`.
- **Any schema change = migration `0026_*.sql` bumping `schema_meta.version` to 26**, with the `migration.rs` list and the `operational_doctor.rs` schema-version assertion updated in the same task. *No task in this plan needs one; if you believe one is required, stop and re-read the task — the tables already exist.*
- **Parity status cells must NOT change without updating the contract tests in the same commit.** The suites are `crates/agentctl/tests/parity_cli.rs`, `crates/agentctl/tests/worktree_reconciliation_contract.rs`, and `crates/agentctl/tests/enterprise_project_authority_contract.rs`. Only `parity_cli.rs` asserts on the `messaging_inbox` / `group_messaging` / `attachments_media` rows; the other two do not mention them. All three messaging rows stay `partial` in this plan.
- **Test gates are narrow.** Always use a single `--test <name>` (or `--lib`) gate scoped to one package with `-p`. Never run workspace-wide `cargo nextest run`. Never run two `nextest` invocations concurrently. Avoid multi-package `-p a -p b` combinations.
- **Before every commit:** `cargo fmt --all` then `cargo clippy --all-targets -- -D warnings` (scoped to the touched packages with `-p` where practical).

---

## Gap Analysis: what p217–p225 already covers

Read this before starting — it is why the plan has four parity tasks and not ten.

**Already delivered, do not rebuild.** Durable direct messages and `POST /api/messages` (p217); direct `send_message` MCP writes with an explicit sender (p218); stdio identity binding with spoof rejection (p219); durable groups, deduplicated membership, group mentions, group history preview/`read_all`, HTTP group admin, MCP `post` and `check_group` (p220); local-file attachment metadata (p221); media stage/fetch (p222); proxy media-cache localization (p223); message/group/cursor import (p224); task import (p225).

**Read cursors are durable already — but they do not advance on the default live read.** agentd models agent-chat's `cursors.json` as per-message read markers: `direct_messages.read_at`, `group_message_reads`, `group_mention_reads`. These are durable, survive restart, and p224 restores them from `cursors.json`. So the hypothesis that "p224 only imported cursors and live reads do not persist" is **false** — `read_direct_inbox` and `read_group_mentions` both write read markers when `drain` is set, inside a transaction. The real cursor gap is the *default*: agent-chat's `GET /api/inbox/:agent` advances unless a `kinds` filter is set, while agentd's `drain` defaults to `false`, so an agent calling `check_inbox` with no arguments is re-delivered the same mail forever. Task 3 closes that, and adds the `kinds` filter that makes a read a deliberate preview.

**Mentions have one real hole: read-time membership.** Mention *authoring* is at parity — agent-chat takes an explicit `mentions` array with no `@`-parsing from message text, and so does agentd's `post`; both dedup the list; both suppress out-of-group mentions at post time (agentd omits them from the delivered `mentions`, agent-chat records them in `suppressedRecipients` — same observable result). DM "mentions" are a non-issue: agent-chat's `send_message` always posts `mentions: []`. The hole is that agent-chat filters mention delivery by *current* membership at read time (`if (!isGroupMember(m.group, agentName)) continue;`) and agentd does not, so a removed member keeps receiving that group's older mentions. Task 4 closes that.

**Dedup is already at parity — this plan adds no dedup task.** Group membership dedup and mention-list dedup landed in p220 (`clean_dedup`). Both inserts use `ON CONFLICT(id) DO NOTHING` and return the existing row, so import and replay are idempotent. agent-chat's inbox dedups DM and mention rows into one map keyed by message id; in agentd a message lives in exactly one of the two tables, so the collision cannot occur. And agent-chat has **no** client-supplied idempotency key on send — ids come from a server-side `reserveNextMsgId` counter — so adding one would be a hardening addition, not parity. Do not add one.

**Field-shape divergence: one, on the DM send path.** agent-chat's `send_message` tool accepts `schema: { kind, version, payload }`. agentd's `post` accepts `schema`, `POST /api/messages` accepts `schema`, the `direct_messages.schema_json` column exists, `InboxMessage` serializes `schema`, and p224's import preserves it — but `tools/send_message.rs` has no `schema` field and hardcodes `None`. Task 2 closes that, and it is a prerequisite for Task 3's `kinds` filter being useful on DMs.

**Notification-gate semantics: one item is messaging-core, the rest are not.** agent-chat's idle delivery gate, `NotificationRouter` cooldowns, and `inbox.read_ack` delivery events all depend on local agent activity state and feed dashboards/relay — M4. But `POST /api/messages/:id/suppress` is agent-callable over the agent-token boundary and is pure message lifecycle: it drops one message from one recipient's unread inbox without draining everything queued behind it. agentd has no equivalent. Task 5 closes that.

---

## Non-Goals (explicitly out of scope for this plan)

These appear as "pending" in the parity notes but belong to later milestones. Do not implement them here.

- Task graph CRUD / coordination semantics — M3 Plan C.
- Matrix and remote-relay message delivery, delivery-event emission for suppression, dashboard message views — M4.
- Cutover, rollback, token provisioning — M5.
- agent-chat's `advance=delivered` group-read mode. Its MCP `check_group` only ever sends `advance=all` or `advance=none`, which agentd already covers; `delivered` is an HTTP-only dashboard affordance and belongs with M4's view work.
- Client-supplied idempotency keys on send. agent-chat generates message ids server-side (`reserveNextMsgId`) and has no send-side dedup, so adding one is not parity. agentd's `ON CONFLICT(id) DO NOTHING` on both inserts already makes the import/replay path idempotent.

---

### Task 1: M3 Plan A hardening carry-overs

Three small items from the M3 Plan A final review, folded into one task because each is a few lines and they share a review context.

1. `agent_repo::update_agent_profile` does read-merge-write across two statements outside a transaction, so two concurrent `PATCH /api/agents/:name/profile` calls can lose one side's keys. Move the read, merge and write inside `BEGIN IMMEDIATE`, mirroring `import_in_transaction`.
2. `project_binding_http::put_binding` takes `Json(body)` as its last argument, so axum deserializes the body *before* the handler runs and a malformed body from an unauthenticated caller gets 400 instead of 401. Authenticate first, parse after.
3. `project_binding_http::authenticate` reimplements the operator bearer check with `strip_prefix("Bearer ")` (case-sensitive) and an untrimmed comparison, diverging from `http::require_operator_bearer` (case-insensitive scheme, trimmed token). Delete the duplicate and call the shared helper.

**Files:**
- Modify: `crates/agentd-store/src/agent_repo.rs:130-168`
- Modify: `crates/agentd-surface/src/http.rs:1628-1664` (make `AuthRejection`, `AuthRejection::into_response`, and `require_operator_bearer` `pub(crate)`)
- Modify: `crates/agentd-surface/src/project_binding_http.rs:55-70` (`put_binding`) and `:107-130` (`authenticate`)
- Test: `crates/agentd-store/tests/agent_registry.rs` (append)
- Test: `crates/agentd-bin/tests/project_binding_http.rs` (append)

**Interfaces:**
- Consumes: nothing from other tasks — this is the first task.
- Produces: `agent_repo::update_agent_profile(pool: &SqlitePool, name: &str, patch: Value, replace: bool) -> Result<Option<AgentRecord>, StoreError>` (signature unchanged, now transactional); `pub(crate) fn require_operator_bearer(auth: &AuthConfig, headers: &HeaderMap) -> Result<(), AuthRejection>` and `pub(crate) enum AuthRejection` in `crates/agentd-surface/src/http.rs`, both reusable by sibling transports. No later task in this plan depends on these.

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/agent_registry.rs`. That file already defines `open_temp()` and `text()` and imports `agent_repo` and `serde_json::json`, so use them directly. Note `RegisterAgent::runtime_profile` is a plain `Value`, not an `Option<Value>`:

```rust
#[tokio::test]
async fn concurrent_profile_merges_do_not_lose_keys() {
    let (store, _dir) = open_temp().await;
    agent_repo::register_agent(
        store.pool(),
        agent_repo::RegisterAgent {
            name: text("codex-a"),
            role: Some(text("agent")),
            capability: None,
            runtime: Some(text("codex")),
            model: None,
            tmux_target: None,
            home_dir: None,
            workdir: None,
            state_dir: None,
            server: None,
            runtime_profile: json!({ "base": 1 }),
        },
    )
    .await
    .expect("register");

    let left = agent_repo::update_agent_profile(store.pool(), "codex-a", json!({ "left": "L" }), false);
    let right =
        agent_repo::update_agent_profile(store.pool(), "codex-a", json!({ "right": "R" }), false);
    let (left, right) = tokio::join!(left, right);
    left.expect("left update").expect("left agent");
    right.expect("right update").expect("right agent");

    let profile = agent_repo::get_agent_profile(store.pool(), "codex-a")
        .await
        .expect("read profile")
        .expect("profile present");
    assert_eq!(profile["base"], 1, "pre-existing key survives: {profile}");
    assert_eq!(profile["left"], "L", "left merge survives: {profile}");
    assert_eq!(profile["right"], "R", "right merge survives: {profile}");
}
```

- [ ] **Step 2: Run the store test to verify it fails**

Run: `cargo nextest run -p agentd-store --test agent_registry concurrent_profile_merges_do_not_lose_keys`
Expected: FAIL — one of `left` or `right` is missing from the merged profile (last writer wins), or the run errors with a `Conflict("agent 'codex-a' changed concurrently")`.

- [ ] **Step 3: Make `update_agent_profile` transactional**

Replace the body of `update_agent_profile` in `crates/agentd-store/src/agent_repo.rs`:

```rust
pub async fn update_agent_profile(
    pool: &SqlitePool,
    name: &str,
    patch: Value,
    replace: bool,
) -> Result<Option<AgentRecord>, StoreError> {
    let name = normalize_name(name)?;
    if !patch.is_object() {
        return Err(StoreError::Invariant(
            "runtime profile must be a JSON object".to_string(),
        ));
    }
    let now = now_unix();

    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = update_profile_in_transaction(&mut connection, &name, &patch, replace, now).await;
    let found = match result {
        Ok(found) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            found
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            return Err(error);
        }
    };
    drop(connection);

    if !found {
        return Ok(None);
    }
    get_agent(pool, &name).await
}

async fn update_profile_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    name: &str,
    patch: &Value,
    replace: bool,
    now: i64,
) -> Result<bool, StoreError> {
    let existing: Option<String> =
        sqlx::query_scalar("SELECT runtime_profile FROM agents WHERE name = ? OR id = ?")
            .bind(name)
            .bind(name)
            .fetch_optional(&mut *connection)
            .await?;
    let Some(existing_profile_text) = existing else {
        return Ok(false);
    };

    let next = if replace {
        patch.clone()
    } else {
        merge_runtime_profile(&existing_profile_text, patch)
    };
    let next_text = serde_json::to_string(&next)?;
    let updated = sqlx::query(
        "UPDATE agents SET runtime_profile = ?, updated_at = ? WHERE name = ? OR id = ?",
    )
    .bind(next_text)
    .bind(now)
    .bind(name)
    .bind(name)
    .execute(&mut *connection)
    .await?;
    if updated.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "agent '{name}' changed concurrently"
        )));
    }
    Ok(true)
}
```

- [ ] **Step 4: Run the store tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test agent_registry`
Expected: PASS, including the pre-existing `update_agent_profile` merge/replace/error tests around `crates/agentd-store/tests/agent_registry.rs:528-565`.

- [ ] **Step 5: Write the failing binding HTTP test**

Append to `crates/agentd-bin/tests/project_binding_http.rs`:

```rust
#[tokio::test]
async fn binding_put_authenticates_before_reading_the_body() {
    let (app, _dir) = app().await;
    let response = app
        .clone()
        .oneshot(
            Request::put("/api/projects/proj-1/binding")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "an unauthenticated caller must never reach body parsing"
    );

    let response = app
        .oneshot(
            Request::put("/api/projects/proj-1/binding")
                .header("authorization", "Bearer operator-secret")
                .header("content-type", "application/json")
                .body(Body::from("{not json"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        response.status(),
        StatusCode::BAD_REQUEST,
        "an authenticated caller with a malformed body gets Invalid -> 400"
    );
}

#[tokio::test]
async fn binding_bearer_check_matches_the_shared_operator_helper() {
    let (app, _dir) = app().await;
    let response = app
        .oneshot(
            Request::get("/api/projects/proj-1/binding")
                .header("authorization", "bearer operator-secret")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_ne!(
        response.status(),
        StatusCode::UNAUTHORIZED,
        "the shared helper treats the auth scheme case-insensitively"
    );
}
```

- [ ] **Step 6: Run the binding test to verify it fails**

Run: `cargo nextest run -p agentd-bin --test project_binding_http`
Expected: FAIL — `binding_put_authenticates_before_reading_the_body` sees 400 on the first request instead of 401, and `binding_bearer_check_matches_the_shared_operator_helper` sees 401 for the lowercase `bearer` scheme.

- [ ] **Step 7: Widen the shared bearer helper's visibility**

In `crates/agentd-surface/src/http.rs`, change these three declarations (leave the bodies unchanged):

```rust
pub(crate) enum AuthRejection {
    BearerRequired,
    LocalOnly,
    AgentTokenRequired,
}

impl AuthRejection {
    pub(crate) fn into_response(self) -> Response {
```

```rust
pub(crate) fn require_operator_bearer(
    auth: &AuthConfig,
    headers: &HeaderMap,
) -> Result<(), AuthRejection> {
```

- [ ] **Step 8: Fix the binding transport**

In `crates/agentd-surface/src/project_binding_http.rs`, add `use axum::body::Bytes;` to the axum import block, replace `put_binding`, and replace `authenticate`:

```rust
async fn put_binding(
    State(state): State<ProjectBindingHttpState>,
    AxumPath(project_id): AxumPath<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    if let Some(response) = authenticate(&state.auth, &headers) {
        return response;
    }
    let body: ProjectBindingBody = match serde_json::from_slice(&body) {
        Ok(body) => body,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "error": format!("invalid binding body: {error}") })),
            )
                .into_response();
        }
    };
    let request = ProjectRoomRepoBindingRequest {
        project_id,
        room_id: body.room_id,
        repository_id: body.repository_id,
        repository_url: body.repository_url,
        default_branch: body.default_branch,
    };
    respond(state.bindings.put_binding(&request).await)
}
```

```rust
/// Returns the rejection response when the bearer token is missing or wrong.
/// Delegates to the shared operator check so this transport cannot drift from
/// `/api/*` on scheme casing or token trimming.
fn authenticate(auth: &AuthConfig, headers: &HeaderMap) -> Option<Response> {
    crate::http::require_operator_bearer(auth, headers)
        .err()
        .map(crate::http::AuthRejection::into_response)
}
```

`Bytes` is an infallible extractor, so it may stay in last position. Delete the now-unused `Json`-body import only if `Json` is no longer referenced — it still is, by `respond` and the error arm, so leave the import list otherwise untouched.

- [ ] **Step 9: Run both HTTP suites to verify they pass**

Run: `cargo nextest run -p agentd-bin --test project_binding_http`
Expected: PASS (5 tests, including the pre-existing `binding_api_declares_reads_and_classifies_errors` and `binding_api_requires_the_operator_bearer_token`).

Then run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS — confirms the visibility widening broke nothing.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-surface --all-targets -- -D warnings
git add crates/agentd-store/src/agent_repo.rs crates/agentd-store/tests/agent_registry.rs crates/agentd-surface/src/http.rs crates/agentd-surface/src/project_binding_http.rs crates/agentd-bin/tests/project_binding_http.rs
git commit -m "fix(m3a): transactional profile merge, auth-before-body, shared bearer check"
```

---

### Task 2: `send_message` carries the structured `schema` object

**Gap.** agent-chat's `send_message` MCP tool accepts `schema: { kind, version, payload }` and forwards it to `POST /api/messages`. agentd's `post` tool has `schema`, `DirectMessageInput` has `schema`, the `direct_messages.schema_json` column exists, `InboxMessage` serializes `schema`, and the p224 import path preserves it — but `tools/send_message.rs` has no `schema` field and hardcodes `schema: None`. A DM sent through MCP therefore loses its structured payload, while the same DM sent through `POST /api/messages` or restored by import keeps it. This also blocks Task 3: a `kinds` filter is useless if agents cannot set `schema.kind` on a DM.

**Files:**
- Modify: `crates/agentd-surface/src/tools/send_message.rs:11-26` (input struct) and `:67-86` (host call)
- Modify: `crates/agentd-bin/src/stdio_mcp.rs:1189-1223` (`send_message_schema`)
- Test: `crates/agentd-surface/tests/tools.rs:78-89` (the `send_input` helper — the file's only `SendMessageInput` literal) and append

**Interfaces:**
- Consumes: `RunHost::post_direct_message(input: DirectMessageInput) -> Result<InboxMessage, CoreError>` and `crate::host::DirectMessageInput { message_id, ts, from, to, message_type, priority, summary, full, reply_to, source, source_room, sender_mxid, trust_level, from_id, schema: Option<Value>, attachments }` — both already exist in `crates/agentd-surface/src/host.rs`.
- Produces: `SendMessageInput` gains `pub schema: Option<Value>` (serde default). Task 3 relies on `InboxMessage.schema` being populated for MCP-sent DMs.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-surface/tests/tools.rs`:

`FakeRunHost::post_direct_message` does not require the sender or target to be registered — the neighbouring `send_message_writes_direct_message_visible_through_check_inbox` test relies on that. The existing `send_input(from_agent, priority)` helper builds a message from `"codex-worker"` to `"codex-reviewer"`, so extend it rather than writing a second literal:

```rust
#[tokio::test]
async fn send_message_preserves_the_structured_schema_object() {
    let host = FakeRunHost::new();
    let mut input = send_input("codex-worker", None);
    input.schema = Some(json!({
        "kind": "task_result",
        "version": 1,
        "payload": { "nodeId": "a", "ok": true }
    }));

    let sent = send_message(&host, input).await.expect("send_message ok");
    assert_eq!(sent.message["schema"]["kind"], "task_result");
    assert_eq!(sent.message["schema"]["payload"]["nodeId"], "a");

    let inbox = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-reviewer".to_string(),
            drain: false,
        },
    )
    .await
    .expect("check_inbox ok");
    assert_eq!(
        inbox.dm[0]["schema"]["kind"], "task_result",
        "delivered DM keeps the schema the sender set: {:?}",
        inbox.dm[0]
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo nextest run -p agentd-surface --test tools send_message_preserves_the_structured_schema_object`
Expected: FAIL to compile — `SendMessageInput` has no field named `schema`.

- [ ] **Step 3: Add the field and forward it**

In `crates/agentd-surface/src/tools/send_message.rs`, add to `SendMessageInput` (after `reply_to`):

```rust
    #[serde(default)]
    pub schema: Option<Value>,
```

and in the `host.post_direct_message(DirectMessageInput { ... })` call, replace `schema: None,` with:

```rust
            schema: input.schema,
```

`normalize_local_attachments(input.attachments)` has already moved one field out of `input` by that point; moving a second, distinct field out is a legal partial move, so no local hoist or `mut` binding is needed.

Then add the field to the test helper `send_input` at `crates/agentd-surface/tests/tools.rs:78-89` — it is the file's only `SendMessageInput` literal, so this one edit keeps every existing send test compiling:

```rust
        reply_to: None,
        attachments: Vec::new(),
        schema: None,
    }
}
```

- [ ] **Step 4: Advertise the field to MCP clients**

In `crates/agentd-bin/src/stdio_mcp.rs`, inside `send_message_schema`'s `"properties"` object, add after `"reply_to": { "type": "string" }`:

```rust
            "reply_to": { "type": "string" },
            "schema": {
                "type": "object",
                "description": "Optional structured message schema, e.g. { kind, version, payload }."
            }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-surface --test tools`
Expected: PASS.

Then run: `cargo nextest run -p agentd-bin --test mcp_stdio`
Expected: PASS — the advertised schema changed but no assertion pins the property list; if one does, extend it to expect `schema`.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentd-surface -p agentd-bin --all-targets -- -D warnings
git add crates/agentd-surface/src/tools/send_message.rs crates/agentd-surface/tests/tools.rs crates/agentd-bin/src/stdio_mcp.rs
git commit -m "feat(messaging): carry structured schema through the send_message MCP path"
```

---

### Task 3: `check_inbox` advances the read cursor by default and accepts a `kinds` filter

**Gap.** agent-chat's `GET /api/inbox/:agent` advances the agent's durable cursor on every unfiltered read ("Reading advances your cursor — messages shown here won't appear again next time"), and turns into a non-advancing preview only when a `kinds` filter is supplied — the backend comment explains why: *"a global inbox cursor cannot safely advance over one kind without implicitly skipping unread messages of other kinds."* agentd's `check_inbox` defaults `drain` to `false` and has no `kinds` parameter, so an agent that calls `check_inbox` with no arguments (which is what the identity-bound stdio session encourages) is re-delivered the same messages forever, and cannot ask for just `task_result`-shaped mail.

The durable read state itself already exists and already survives restart (`direct_messages.read_at`, `group_mention_reads`), so this is a defaults-and-filter change only. The filter is applied in the surface layer, not the store, precisely because agent-chat's rule is "a filtered read never advances" — so no filtered row can ever be marked read.

**Files:**
- Modify: `crates/agentd-surface/src/tools/check_inbox.rs` (whole file)
- Modify: `crates/agentd-surface/src/http.rs:824-846` (`InboxQuery`, `get_inbox`)
- Modify: `crates/agentd-bin/src/stdio_mcp.rs:1226-1245` (`check_inbox_schema`)
- Test: `crates/agentd-surface/tests/tools.rs` (12 existing literals + append)
- Test: `crates/agentd-bin/tests/daemon_http.rs:829` and `:884` (two reads of the same agent in one test)

**Interfaces:**
- Consumes: `RunHost::check_inbox(&self, agent_id: &str, drain: bool) -> Result<Vec<InboxMessage>, CoreError>` (unchanged); `InboxMessage.schema: Option<Value>` populated for MCP-sent DMs by Task 2.
- Produces: `CheckInboxInput { pub agent_id: String, pub drain: bool, pub kinds: Vec<String> }` where `drain` deserializes to `true` when absent and `kinds` deserializes to an empty `Vec` when absent. `CheckInboxOutput` is unchanged (`messages`, `dm`, `group`).

- [ ] **Step 1: Write the failing tests**

Append to `crates/agentd-surface/tests/tools.rs`:

```rust
#[tokio::test]
async fn check_inbox_advances_the_cursor_by_default_like_agent_chat() {
    let host = FakeRunHost::new();
    host.push_inbox_message(inbox_message("msg_default_drain"));

    let deserialized: CheckInboxInput =
        serde_json::from_value(serde_json::json!({ "agent_id": "codex-worker" }))
            .expect("check_inbox args without drain");
    assert!(
        deserialized.drain,
        "an unfiltered check_inbox consumes, matching agent-chat's GET /api/inbox/:agent"
    );
    assert!(deserialized.kinds.is_empty(), "no filter by default");

    let first = check_inbox(&host, deserialized).await.expect("first read");
    assert_eq!(first.dm.len(), 1);

    let second = check_inbox(
        &host,
        serde_json::from_value(serde_json::json!({ "agent_id": "codex-worker" }))
            .expect("check_inbox args"),
    )
    .await
    .expect("second read");
    assert!(
        second.messages.is_empty(),
        "the default read advanced the durable cursor: {:?}",
        second.messages
    );
}

#[tokio::test]
async fn check_inbox_kinds_filter_is_a_non_advancing_preview() {
    let host = FakeRunHost::new();
    let mut matching = inbox_message("msg_kind_match");
    matching.schema = Some(json!({ "kind": "task_result", "version": 1 }));
    let mut other = inbox_message("msg_kind_other");
    other.schema = Some(json!({ "kind": "status_report", "version": 1 }));
    let unschemad = inbox_message("msg_kind_none");
    host.push_inbox_message(matching);
    host.push_inbox_message(other);
    host.push_inbox_message(unschemad);

    let filtered = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-worker".to_string(),
            drain: true,
            kinds: vec!["task_result".to_string()],
        },
    )
    .await
    .expect("filtered read");
    assert_eq!(filtered.messages.len(), 1, "{:?}", filtered.messages);
    assert_eq!(filtered.messages[0]["id"], "msg_kind_match");

    let after = check_inbox(
        &host,
        CheckInboxInput {
            agent_id: "codex-worker".to_string(),
            drain: false,
            kinds: Vec::new(),
        },
    )
    .await
    .expect("unfiltered preview");
    assert_eq!(
        after.messages.len(),
        3,
        "a filtered read never advances the cursor, even with drain=true: {:?}",
        after.messages
    );
}
```

`inbox_message(id)` is the existing helper at `crates/agentd-surface/tests/tools.rs:53-76`; it returns an `InboxMessage` addressed to `codex-worker` with `schema: None`, which is exactly what these tests need.

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p agentd-surface --test tools check_inbox`
Expected: FAIL to compile — `CheckInboxInput` has no field named `kinds`, and the deserialization test cannot see a `drain` default.

- [ ] **Step 3: Rewrite `check_inbox`**

Replace the contents of `crates/agentd-surface/src/tools/check_inbox.rs` below the imports:

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct CheckInboxInput {
    pub agent_id: String,
    /// Whether this read advances the agent's durable read cursor. Defaults to
    /// `true`, matching agent-chat's `GET /api/inbox/:agent`. Forced to `false`
    /// whenever `kinds` is set: a single cursor cannot advance over one kind
    /// without silently skipping unread messages of every other kind.
    #[serde(default = "default_drain")]
    pub drain: bool,
    /// Optional `schema.kind` filter. When non-empty the read is a preview.
    #[serde(default)]
    pub kinds: Vec<String>,
}

const fn default_drain() -> bool {
    true
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckInboxOutput {
    pub messages: Vec<Value>,
    pub dm: Vec<Value>,
    pub group: Vec<Value>,
}

/// Pull the agent's inbox.
///
/// # Errors
/// [`SurfaceError`] on host/store failures or JSON encoding failures.
pub async fn check_inbox(
    host: &dyn RunHost,
    input: CheckInboxInput,
) -> Result<CheckInboxOutput, SurfaceError> {
    let kinds = input
        .kinds
        .iter()
        .map(|kind| kind.trim())
        .filter(|kind| !kind.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let advance = kinds.is_empty() && input.drain;

    let messages = host.check_inbox(&input.agent_id, advance).await?;
    let encoded = messages
        .into_iter()
        .filter(|message| matches_kinds(message.schema.as_ref(), &kinds))
        .map(|message| {
            serde_json::to_value(message)
                .map_err(|e| SurfaceError::Internal(format!("encode inbox message: {e}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let dm = encoded
        .iter()
        .filter(|message| message.get("group").is_none_or(Value::is_null))
        .cloned()
        .collect::<Vec<_>>();
    let group = encoded
        .iter()
        .filter(|message| message.get("group").is_some_and(|value| !value.is_null()))
        .cloned()
        .collect::<Vec<_>>();
    Ok(CheckInboxOutput {
        messages: encoded,
        dm,
        group,
    })
}

/// agent-chat's `messageMatchesKinds`: an empty filter matches everything; a
/// non-empty filter matches only messages carrying a listed `schema.kind`.
fn matches_kinds(schema: Option<&Value>, kinds: &[String]) -> bool {
    if kinds.is_empty() {
        return true;
    }
    schema
        .and_then(|schema| schema.get("kind"))
        .and_then(Value::as_str)
        .is_some_and(|kind| kinds.iter().any(|wanted| wanted == kind))
}
```

Keep the existing `use` block at the top of the file unchanged: it already brings in `serde::{Deserialize, Serialize}`, `serde_json::Value`, `SurfaceError` and `RunHost`, and the filter closure never names `InboxMessage`, so no new import is needed.

- [ ] **Step 4: Update the 12 existing `CheckInboxInput` literals**

Every pre-existing literal in `crates/agentd-surface/tests/tools.rs` now needs the new field. Each sits on a `drain: <bool>,` line, and in this file `drain:` appears only inside `CheckInboxInput` literals, so this is a mechanical insert. The negative lookahead makes it idempotent and skips the literals written by hand in Step 1, which already carry `kinds`:

```bash
perl -0pi -e 's/^([ \t]*)drain: (true|false),\n(?![ \t]*kinds:)/$1drain: $2,\n$1kinds: Vec::new(),\n/gm' crates/agentd-surface/tests/tools.rs
```

Verify every literal now has both fields:

```bash
test "$(grep -c 'drain:' crates/agentd-surface/tests/tools.rs)" = "$(grep -c 'kinds:' crates/agentd-surface/tests/tools.rs)" && echo balanced
```

Expected: `balanced`.

- [ ] **Step 5: Update the HTTP query type**

In `crates/agentd-surface/src/http.rs`, replace `InboxQuery` and the `check_inbox` call inside `get_inbox`:

```rust
#[derive(Debug, Deserialize)]
struct InboxQuery {
    /// Defaults to `true` so an unqualified `GET /api/inbox/:agent` consumes,
    /// matching agent-chat. Pass `?drain=false` for a preview.
    #[serde(default = "default_inbox_drain")]
    drain: bool,
    /// Comma-separated `schema.kind` filter, as in agent-chat's `?kinds=`.
    #[serde(default)]
    kinds: Option<String>,
}

const fn default_inbox_drain() -> bool {
    true
}

async fn get_inbox(
    State(state): State<AppState>,
    AxumPath(agent): AxumPath<String>,
    Query(query): Query<InboxQuery>,
) -> Response {
    let kinds = query
        .kinds
        .as_deref()
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    match check_inbox(
        state.host.as_ref(),
        CheckInboxInput {
            agent_id: agent,
            drain: query.drain,
            kinds,
        },
    )
    .await
    {
        Ok(out) => Json(out).into_response(),
        Err(e) => surface_error_response(e),
    }
}
```

- [ ] **Step 6: Advertise the parameters to MCP clients**

In `crates/agentd-bin/src/stdio_mcp.rs`, replace `check_inbox_schema`'s `"properties"` entries for `drain` and add `kinds`:

```rust
            "drain": {
                "type": "boolean",
                "default": true,
                "description": "Advance your read cursor. Defaults to true: messages returned by an unfiltered read will not appear again."
            },
            "kinds": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Optional schema.kind filter. When set, only matching unread messages are returned and the read cursor is not advanced."
            }
```

Also update the tool description in `crates/agentd-surface/src/mcp_server.rs:52-54`:

```rust
        ToolDescriptor {
            name: "check_inbox",
            description: "Pull durable direct messages and group mentions for this agent. Reading advances your cursor unless a kinds filter is set.",
        },
```

- [ ] **Step 7: Fix the daemon test that reads one agent's inbox twice**

`daemon_router_task_graph_scheduler_routes_and_releases_nodes` in `crates/agentd-bin/tests/daemon_http.rs` reads `/api/inbox/cod1` at line 829 and again at line 884, asserting the second read returns both messages. That assertion is about dispatch content, not cursor semantics, so make both reads explicit previews. Change line 829 from:

```rust
    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/cod1").await;
```

to:

```rust
    let (inbox_status, inbox_body) = get(app.clone(), "/api/inbox/cod1?drain=false").await;
```

and line 884 from:

```rust
    let (inbox_status, inbox_body) = get(app, "/api/inbox/cod1").await;
```

to:

```rust
    let (inbox_status, inbox_body) = get(app, "/api/inbox/cod1?drain=false").await;
```

The other `/api/inbox/` reads in that file (lines 560, 671, 706, 1899, 1966) each read a given agent exactly once, so they are unaffected — leave them alone. The same is true of the five reads in `crates/agentd-surface/tests/http.rs`.

- [ ] **Step 8: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-surface --test tools`
Expected: PASS.

Then run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 9: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentd-surface -p agentd-bin --all-targets -- -D warnings
git add crates/agentd-surface/src/tools/check_inbox.rs crates/agentd-surface/src/http.rs crates/agentd-surface/src/mcp_server.rs crates/agentd-surface/tests/tools.rs crates/agentd-bin/src/stdio_mcp.rs crates/agentd-bin/tests/daemon_http.rs
git commit -m "feat(messaging): advance the inbox cursor by default and add the kinds preview filter"
```

---

### Task 4: group mentions are delivered only to current group members

**Gap.** agent-chat's `getUnreadInboxMessages` skips any group mention whose group the agent is not currently a member of (`if (!isGroupMember(m.group, agentName)) continue;`). agentd's `message_repo::read_group_mentions` selects every unread `group_messages` row in the database and filters in Rust on the `mentions_json` array only — there is no join to `group_members`. Membership is checked at *post* time (`tools::post::resolve_mentions` drops non-members into `delivery.suppressed`), so the divergence is invisible until membership changes after the fact: an agent removed from a group by `POST /api/groups/:name/members` keeps receiving that group's older mentions in `check_inbox` forever, and imported agent-chat mentions of non-members are delivered even though agent-chat would have skipped them. That is both a parity break and a scope leak — group content reaching someone who has been removed from the group.

**Files:**
- Modify: `crates/agentd-store/src/message_repo.rs:563-607` (`read_group_mentions`)
- Test: `crates/agentd-store/tests/messages.rs` (append)

**Interfaces:**
- Consumes: the `group_members(group_name, agent_name)` table from migration `0006_group_messages.sql`; `message_repo::read_agent_inbox(pool, agent_id, InboxReadOptions { drain }) -> Result<AgentInboxReadResult, StoreError>` (signature unchanged).
- Produces: nothing new. `AgentInboxReadResult.group` now contains only mentions from groups the reader currently belongs to.

- [ ] **Step 1: Write the failing test**

Append to `crates/agentd-store/tests/messages.rs`:

```rust
#[tokio::test]
async fn group_mentions_are_scoped_to_current_group_membership() {
    let (store, _dir) = open_temp().await;
    message_repo::create_group(
        store.pool(),
        message_repo::GroupCreateInput {
            name: text("factory"),
            members: vec![text("codex-a"), text("codex-b")],
        },
    )
    .await
    .expect("create group");

    message_repo::insert_group_message(store.pool(), group_message("mention b", &["codex-b"]))
        .await
        .expect("insert mention");

    let before = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read while a member");
    assert_eq!(before.group.len(), 1, "a member sees the mention");

    message_repo::update_group_members(store.pool(), "factory", &[], &[text("codex-b")])
        .await
        .expect("remove member");

    let after = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read after removal");
    assert!(
        after.group.is_empty(),
        "a removed member stops receiving that group's mentions: {:?}",
        after.group
    );

    let still_member = message_repo::read_agent_inbox(
        store.pool(),
        "codex-a",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read as remaining member");
    assert!(
        still_member.group.is_empty(),
        "codex-a was never mentioned: {:?}",
        still_member.group
    );
}

#[tokio::test]
async fn group_mentions_for_an_unknown_group_are_not_delivered() {
    let (store, _dir) = open_temp().await;
    message_repo::create_group(
        store.pool(),
        message_repo::GroupCreateInput {
            name: text("factory"),
            members: vec![text("codex-a")],
        },
    )
    .await
    .expect("create group");
    message_repo::insert_group_message(store.pool(), group_message("mention b", &["codex-b"]))
        .await
        .expect("insert mention of a non-member");

    let inbox = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read");
    assert!(
        inbox.group.is_empty(),
        "an imported mention of a non-member is not delivered: {:?}",
        inbox.group
    );
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo nextest run -p agentd-store --test messages group_mentions`
Expected: FAIL — both `after.group` and `inbox.group` contain one message, because `read_group_mentions` never consults `group_members`.

- [ ] **Step 3: Scope the mention query to current membership**

In `crates/agentd-store/src/message_repo.rs`, replace the query inside `read_group_mentions`. Group names are compared case-insensitively to match `group_has_member` / `clean_dedup` behaviour elsewhere in the messaging surface:

```rust
async fn read_group_mentions(
    pool: &SqlitePool,
    agent_id: &str,
    options: InboxReadOptions,
) -> Result<Vec<GroupMessageRecord>, StoreError> {
    let mut tx = pool.begin().await?;
    let rows = sqlx::query(
        group_message_select_sql(
            "WHERE id NOT IN (SELECT message_id FROM group_mention_reads WHERE agent_name = ?) \
             AND group_name IN ( \
                 SELECT group_name FROM group_members \
                 WHERE agent_name = ? COLLATE NOCASE \
             ) \
             ORDER BY ts, rowid",
        )
        .as_str(),
    )
    .bind(agent_id)
    .bind(agent_id)
    .fetch_all(&mut *tx)
    .await?;
    let messages = rows
        .iter()
        .map(row_to_group_message)
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|message| {
            message
                .mentions
                .iter()
                .any(|mention| mention.eq_ignore_ascii_case(agent_id))
        })
        .collect::<Vec<_>>();
    if options.drain && !messages.is_empty() {
        let read_at = now_unix();
        for message in &messages {
            sqlx::query(
                "INSERT OR IGNORE INTO group_mention_reads (agent_name, message_id, read_at) \
                 VALUES (?, ?, ?)",
            )
            .bind(agent_id)
            .bind(&message.id)
            .bind(read_at)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(messages)
}
```

- [ ] **Step 4: Run the store tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test messages`
Expected: PASS, including the pre-existing `group mention` tests around `crates/agentd-store/tests/messages.rs:323-370` (their fixtures create the `factory` group with `codex-b` as a member, so they remain valid).

- [ ] **Step 5: Run the dependent suites**

Run: `cargo nextest run -p agentd-store --test agent_chat_import`
Expected: PASS — cursor import writes `group_mention_reads` rows and creates groups from `groups.json`, so the membership predicate must not break restored read state.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store --all-targets -- -D warnings
git add crates/agentd-store/src/message_repo.rs crates/agentd-store/tests/messages.rs
git commit -m "fix(messaging): deliver group mentions only to current group members"
```

---

### Task 5: per-recipient message suppression

**Gap.** agent-chat exposes `POST /api/messages/:id/suppress` (agent-token authenticated), which removes exactly one message from exactly one recipient's unread inbox without touching that agent's other unread state. It is the escape hatch for a message an agent cannot process: without it the only way to clear one message is to drain the whole inbox, which loses everything else queued behind it. agentd has no equivalent. This is messaging-core rather than dashboard surface — the caller is the agent, over the agent-token boundary.

**Design decision to state in review:** agentd models read state as per-recipient markers (`direct_messages.read_at`, `group_mention_reads`), not as a monotonic cursor plus a `suppressedRecipients` array. Suppression is therefore expressed as "mark this one message read for this one agent", which produces agent-chat's observable behaviour with no schema change. The consequence is that agentd does not record *why* a message left an inbox; agent-chat's `message.suppressed` delivery event carries that reason, and delivery events are M4 work.

**Files:**
- Modify: `crates/agentd-store/src/message_repo.rs` (append `SuppressionOutcome` + `suppress_message_for_agent`)
- Modify: `crates/agentd-surface/src/host.rs` (append `SuppressionOutcome` + a `RunHost` method)
- Modify: `crates/agentd-surface/src/http.rs` (route + handler)
- Modify: `crates/agentd-surface/src/test_support.rs` (`FakeRunHost` impl)
- Modify: `crates/agentd-bin/src/host.rs` (production impl, near `check_inbox` at `:2568`)
- Test: `crates/agentd-store/tests/messages.rs` (append)
- Test: `crates/agentd-surface/tests/http.rs` (append)

**Interfaces:**
- Consumes: `direct_messages(id, to_agent, read_at)`, `group_messages(id, group_name, mentions_json)`, `group_members(group_name, agent_name)`, `group_mention_reads(agent_name, message_id, read_at)` — all existing.
- Produces:
  - `agentd_store::message_repo::SuppressionOutcome { Suppressed, AlreadySuppressed, NotDeliverable, NotFound }` (derives `Debug, Clone, Copy, PartialEq, Eq`).
  - `agentd_store::message_repo::suppress_message_for_agent(pool: &SqlitePool, message_id: &str, agent_id: &str) -> Result<SuppressionOutcome, StoreError>`.
  - `agentd_surface::host::SuppressionOutcome` (same four variants, same derives) and `RunHost::suppress_message(&self, message_id: &str, agent_id: &str) -> Result<SuppressionOutcome, CoreError>`.
  - `POST /api/messages/:id/suppress` with body `{ "agent": "<name>" }`, responding `200 {"ok":true,"suppressed":<bool>,"message_id":"<id>","agent":"<name>"}`, `400` for a missing `agent` or a message not deliverable to that agent, `404` for an unknown message.

- [ ] **Step 1: Write the failing store test**

Append to `crates/agentd-store/tests/messages.rs`:

```rust
#[tokio::test]
async fn suppressing_a_direct_message_clears_only_that_message() {
    let (store, _dir) = open_temp().await;
    message_repo::insert_direct_message(store.pool(), direct_message("msg_keep"))
        .await
        .expect("insert keeper");
    message_repo::insert_direct_message(store.pool(), direct_message("msg_drop"))
        .await
        .expect("insert dropped");

    let outcome = message_repo::suppress_message_for_agent(store.pool(), "msg_drop", "codex-worker")
        .await
        .expect("suppress");
    assert_eq!(outcome, message_repo::SuppressionOutcome::Suppressed);

    let replay = message_repo::suppress_message_for_agent(store.pool(), "msg_drop", "codex-worker")
        .await
        .expect("replayed suppress");
    assert_eq!(replay, message_repo::SuppressionOutcome::AlreadySuppressed);

    let inbox = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read");
    assert_eq!(inbox.len(), 1, "{inbox:?}");
    assert_eq!(inbox[0].id, "msg_keep");
}

#[tokio::test]
async fn suppression_classifies_unknown_and_undeliverable_messages() {
    let (store, _dir) = open_temp().await;
    message_repo::insert_direct_message(store.pool(), direct_message("msg_direct_1"))
        .await
        .expect("insert");

    assert_eq!(
        message_repo::suppress_message_for_agent(store.pool(), "msg_missing", "codex-worker")
            .await
            .expect("unknown id"),
        message_repo::SuppressionOutcome::NotFound
    );
    assert_eq!(
        message_repo::suppress_message_for_agent(store.pool(), "msg_direct_1", "codex-other")
            .await
            .expect("wrong recipient"),
        message_repo::SuppressionOutcome::NotDeliverable
    );
}

#[tokio::test]
async fn suppressing_a_group_mention_clears_only_that_mention() {
    let (store, _dir) = open_temp().await;
    message_repo::create_group(
        store.pool(),
        message_repo::GroupCreateInput {
            name: text("factory"),
            members: vec![text("codex-a"), text("codex-b")],
        },
    )
    .await
    .expect("create group");
    let keep = message_repo::insert_group_message(
        store.pool(),
        group_message("mention keep", &["codex-b"]),
    )
    .await
    .expect("insert keeper");
    let drop = message_repo::insert_group_message(
        store.pool(),
        group_message("mention drop", &["codex-b"]),
    )
    .await
    .expect("insert dropped");

    assert_eq!(
        message_repo::suppress_message_for_agent(store.pool(), &drop.id, "codex-b")
            .await
            .expect("suppress mention"),
        message_repo::SuppressionOutcome::Suppressed
    );
    assert_eq!(
        message_repo::suppress_message_for_agent(store.pool(), &keep.id, "codex-a")
            .await
            .expect("unmentioned member"),
        message_repo::SuppressionOutcome::NotDeliverable
    );

    let inbox = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("read");
    assert_eq!(inbox.group.len(), 1, "{:?}", inbox.group);
    assert_eq!(inbox.group[0].id, keep.id);
}
```

- [ ] **Step 2: Run the store tests to verify they fail**

Run: `cargo nextest run -p agentd-store --test messages suppress`
Expected: FAIL to compile — `message_repo::suppress_message_for_agent` and `message_repo::SuppressionOutcome` do not exist.

- [ ] **Step 3: Implement the store operation**

Append to `crates/agentd-store/src/message_repo.rs`:

```rust
/// What a per-recipient suppression request did. `NotDeliverable` means the
/// message exists but was never addressed to this agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionOutcome {
    Suppressed,
    AlreadySuppressed,
    NotDeliverable,
    NotFound,
}

/// Drop one message from one recipient's unread inbox. agentd models read
/// state as per-recipient markers, so suppression is expressed as marking this
/// single message read for this single agent; no other unread state moves.
///
/// # Errors
/// [`StoreError`] on validation or store failure.
pub async fn suppress_message_for_agent(
    pool: &SqlitePool,
    message_id: &str,
    agent_id: &str,
) -> Result<SuppressionOutcome, StoreError> {
    let message_id = required(message_id.to_string(), "message id required")?;
    let agent_id = required(agent_id.to_string(), "agent id required")?;

    let mut connection = pool.acquire().await?;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut *connection)
        .await?;
    let result = suppress_in_transaction(&mut connection, &message_id, &agent_id).await;
    match result {
        Ok(outcome) => {
            sqlx::query("COMMIT").execute(&mut *connection).await?;
            Ok(outcome)
        }
        Err(error) => {
            let _ = sqlx::query("ROLLBACK").execute(&mut *connection).await;
            Err(error)
        }
    }
}

async fn suppress_in_transaction(
    connection: &mut sqlx::SqliteConnection,
    message_id: &str,
    agent_id: &str,
) -> Result<SuppressionOutcome, StoreError> {
    let direct: Option<(String, Option<i64>)> =
        sqlx::query_as("SELECT to_agent, read_at FROM direct_messages WHERE id = ?")
            .bind(message_id)
            .fetch_optional(&mut *connection)
            .await?;
    if let Some((to_agent, read_at)) = direct {
        if !to_agent.eq_ignore_ascii_case(agent_id) {
            return Ok(SuppressionOutcome::NotDeliverable);
        }
        if read_at.is_some() {
            return Ok(SuppressionOutcome::AlreadySuppressed);
        }
        let updated = sqlx::query(
            "UPDATE direct_messages SET read_at = ? WHERE id = ? AND read_at IS NULL",
        )
        .bind(now_unix())
        .bind(message_id)
        .execute(&mut *connection)
        .await?;
        if updated.rows_affected() != 1 {
            return Err(StoreError::Conflict(format!(
                "direct message '{message_id}' changed concurrently"
            )));
        }
        return Ok(SuppressionOutcome::Suppressed);
    }

    let group: Option<(String, String)> =
        sqlx::query_as("SELECT group_name, mentions_json FROM group_messages WHERE id = ?")
            .bind(message_id)
            .fetch_optional(&mut *connection)
            .await?;
    let Some((group_name, mentions_json)) = group else {
        return Ok(SuppressionOutcome::NotFound);
    };

    let mentions: Vec<String> = serde_json::from_str(&mentions_json)?;
    let mentioned = mentions
        .iter()
        .any(|mention| mention.eq_ignore_ascii_case(agent_id));
    let member: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM group_members \
         WHERE group_name = ? COLLATE NOCASE AND agent_name = ? COLLATE NOCASE",
    )
    .bind(&group_name)
    .bind(agent_id)
    .fetch_optional(&mut *connection)
    .await?;
    if !mentioned || member.is_none() {
        return Ok(SuppressionOutcome::NotDeliverable);
    }

    let already: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM group_mention_reads WHERE agent_name = ? AND message_id = ?",
    )
    .bind(agent_id)
    .bind(message_id)
    .fetch_optional(&mut *connection)
    .await?;
    if already.is_some() {
        return Ok(SuppressionOutcome::AlreadySuppressed);
    }
    let inserted = sqlx::query(
        "INSERT INTO group_mention_reads (agent_name, message_id, read_at) VALUES (?, ?, ?)",
    )
    .bind(agent_id)
    .bind(message_id)
    .bind(now_unix())
    .execute(&mut *connection)
    .await?;
    if inserted.rows_affected() != 1 {
        return Err(StoreError::Conflict(format!(
            "group mention '{message_id}' changed concurrently"
        )));
    }
    Ok(SuppressionOutcome::Suppressed)
}
```

- [ ] **Step 4: Run the store tests to verify they pass**

Run: `cargo nextest run -p agentd-store --test messages`
Expected: PASS.

- [ ] **Step 5: Write the failing HTTP test**

Append to `crates/agentd-surface/tests/http.rs`:

```rust
#[tokio::test]
async fn http_suppress_drops_one_message_for_one_recipient() {
    let app = app(FakeRunHost::new());
    for agent in ["codex-a", "codex-b"] {
        register_agent(app.clone(), agent).await;
    }

    let mut ids = Vec::new();
    for summary in ["keep", "drop"] {
        let sent = post(
            app.clone(),
            "/api/messages",
            &json!({
                "from": "codex-a",
                "to": "codex-b",
                "summary": summary,
                "full": summary
            })
            .to_string(),
        )
        .await;
        assert_eq!(sent.status(), StatusCode::CREATED);
        let sent: Value = serde_json::from_str(&body_string(sent).await).expect("json");
        ids.push(
            sent["message"]["id"]
                .as_str()
                .expect("message id")
                .to_string(),
        );
    }

    let suppressed = post(
        app.clone(),
        &format!("/api/messages/{}/suppress", ids[1]),
        &json!({ "agent": "codex-b" }).to_string(),
    )
    .await;
    assert_eq!(suppressed.status(), StatusCode::OK);
    let suppressed: Value = serde_json::from_str(&body_string(suppressed).await).expect("json");
    assert_eq!(suppressed["ok"], true);
    assert_eq!(suppressed["suppressed"], true);

    let replay = post(
        app.clone(),
        &format!("/api/messages/{}/suppress", ids[1]),
        &json!({ "agent": "codex-b" }).to_string(),
    )
    .await;
    assert_eq!(replay.status(), StatusCode::OK);
    let replay: Value = serde_json::from_str(&body_string(replay).await).expect("json");
    assert_eq!(replay["suppressed"], false, "replay is idempotent");

    let missing = post(
        app.clone(),
        "/api/messages/msg_nope/suppress",
        &json!({ "agent": "codex-b" }).to_string(),
    )
    .await;
    assert_eq!(missing.status(), StatusCode::NOT_FOUND);

    let wrong = post(
        app.clone(),
        &format!("/api/messages/{}/suppress", ids[0]),
        &json!({ "agent": "codex-a" }).to_string(),
    )
    .await;
    assert_eq!(wrong.status(), StatusCode::BAD_REQUEST);

    let inbox = get(app, "/api/inbox/codex-b?drain=false").await;
    assert_eq!(inbox.status(), StatusCode::OK);
    let inbox: Value = serde_json::from_str(&body_string(inbox).await).expect("json");
    let dm = inbox["dm"].as_array().expect("dm");
    assert_eq!(dm.len(), 1, "{dm:?}");
    assert_eq!(dm[0]["summary"], "keep");
}
```

`app(FakeRunHost::new())`, `register_agent(app, name)`, `post(app, path, &body)`, `get(app, path)` and `body_string(response)` are the existing helpers in `crates/agentd-surface/tests/http.rs` — the neighbouring `http_group_messages_preview_and_advance_cursor` test uses all of them in exactly this form.

- [ ] **Step 6: Run the HTTP test to verify it fails**

Run: `cargo nextest run -p agentd-surface --test http http_suppress_drops_one_message_for_one_recipient`
Expected: FAIL — the route is unmounted, so the first suppress returns 404 and the `ok` assertion fails.

- [ ] **Step 7: Add the port method and its two implementations**

In `crates/agentd-surface/src/host.rs`, add next to the other messaging types:

```rust
/// What a per-recipient suppression request did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuppressionOutcome {
    Suppressed,
    AlreadySuppressed,
    NotDeliverable,
    NotFound,
}
```

and add to the `RunHost` trait, after `check_inbox`:

```rust
    /// Drop one message from one recipient's unread inbox without moving that
    /// agent's other read state.
    ///
    /// # Errors
    /// [`CoreError`] on validation or store failure.
    async fn suppress_message(
        &self,
        message_id: &str,
        agent_id: &str,
    ) -> Result<SuppressionOutcome, CoreError>;
```

In `crates/agentd-bin/src/host.rs`, add after the `check_inbox` impl (around line 2586):

```rust
    async fn suppress_message(
        &self,
        message_id: &str,
        agent_id: &str,
    ) -> Result<SurfaceSuppressionOutcome, CoreError> {
        let outcome =
            message_repo::suppress_message_for_agent(self.store.pool(), message_id, agent_id)
                .await?;
        Ok(match outcome {
            message_repo::SuppressionOutcome::Suppressed => SurfaceSuppressionOutcome::Suppressed,
            message_repo::SuppressionOutcome::AlreadySuppressed => {
                SurfaceSuppressionOutcome::AlreadySuppressed
            }
            message_repo::SuppressionOutcome::NotDeliverable => {
                SurfaceSuppressionOutcome::NotDeliverable
            }
            message_repo::SuppressionOutcome::NotFound => SurfaceSuppressionOutcome::NotFound,
        })
    }
```

Import it the way the file already aliases surface types, e.g. `use agentd_surface::host::SuppressionOutcome as SurfaceSuppressionOutcome;` alongside the existing `SurfaceInboxMessage` / `SurfaceGroupReadResult` aliases.

In `crates/agentd-surface/src/test_support.rs`, add to the `FakeRunHost` impl next to its `check_inbox` (around line 1954). The fake stores `InboxEntry { message, read }` in `self.inbox` and `(agent, message_id)` pairs in `self.group_mention_reads`:

```rust
    async fn suppress_message(
        &self,
        message_id: &str,
        agent_id: &str,
    ) -> Result<SuppressionOutcome, CoreError> {
        let agent_id = normalize_agent_name(agent_id)?;
        let mut inbox = self.inbox.lock().expect("inbox lock");
        let mut mention_reads = self
            .group_mention_reads
            .lock()
            .expect("group_mention_reads lock");
        let Some(entry) = inbox
            .iter_mut()
            .find(|entry| entry.message.id == message_id)
        else {
            return Ok(SuppressionOutcome::NotFound);
        };
        if entry.message.group.is_some() {
            if !entry
                .message
                .mentions
                .iter()
                .any(|mention| mention.eq_ignore_ascii_case(&agent_id))
            {
                return Ok(SuppressionOutcome::NotDeliverable);
            }
            if !mention_reads.insert((agent_id, entry.message.id.clone())) {
                return Ok(SuppressionOutcome::AlreadySuppressed);
            }
            return Ok(SuppressionOutcome::Suppressed);
        }
        if !entry.message.to.eq_ignore_ascii_case(&agent_id) {
            return Ok(SuppressionOutcome::NotDeliverable);
        }
        if entry.read {
            return Ok(SuppressionOutcome::AlreadySuppressed);
        }
        entry.read = true;
        Ok(SuppressionOutcome::Suppressed)
    }
```

Add `SuppressionOutcome` to that file's `use crate::host::{...}` list.

- [ ] **Step 8: Mount the route**

In `crates/agentd-surface/src/http.rs`, add to the router next to the `/api/messages` route (around line 152):

```rust
        .route("/api/messages/:id/suppress", post(suppress_message))
```

and add the handler alongside `post_message`:

```rust
#[derive(Debug, Deserialize)]
struct SuppressReq {
    agent: String,
}

async fn suppress_message(
    State(state): State<AppState>,
    AxumPath(message_id): AxumPath<String>,
    headers: HeaderMap,
    Json(req): Json<SuppressReq>,
) -> Response {
    let Some(agent) = clean_required_text(&req.agent) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "agent required" })),
        )
            .into_response();
    };
    if let Err(err) = require_agent_token(&state.auth, &headers, &agent) {
        return err.into_response();
    }
    match state.host.suppress_message(&message_id, &agent).await {
        Ok(SuppressionOutcome::Suppressed) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "suppressed": true,
                "message_id": message_id,
                "agent": agent
            })),
        )
            .into_response(),
        Ok(SuppressionOutcome::AlreadySuppressed) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "suppressed": false,
                "message_id": message_id,
                "agent": agent
            })),
        )
            .into_response(),
        Ok(SuppressionOutcome::NotDeliverable) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": format!("message {message_id} is not deliverable to {agent}")
            })),
        )
            .into_response(),
        Ok(SuppressionOutcome::NotFound) => (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": format!("message not found: {message_id}") })),
        )
            .into_response(),
        Err(e) => agent_error_response(e),
    }
}
```

Add `SuppressionOutcome` to the file's `use crate::host::{...}` list. `clean_required_text`, `require_agent_token` and `agent_error_response` already exist in this file.

Note for review: unlike Task 1's binding `PUT`, this handler deliberately keeps `Json` extraction ahead of the auth check, because the agent whose token is checked is named *in the body*. agent-chat's `POST /api/messages/:id/suppress` has the same shape. A malformed body therefore returns 400 before any token is examined, which leaks nothing — there is no per-message information in that response.

- [ ] **Step 9: Run the tests to verify they pass**

Run: `cargo nextest run -p agentd-surface --test http`
Expected: PASS.

Then run: `cargo nextest run -p agentd-surface --test tools`
Expected: PASS — the new trait method must not have broken `FakeRunHost`.

Then run: `cargo nextest run -p agentd-bin --test daemon_http`
Expected: PASS.

- [ ] **Step 10: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentd-store -p agentd-surface -p agentd-bin --all-targets -- -D warnings
git add crates/agentd-store/src/message_repo.rs crates/agentd-store/tests/messages.rs crates/agentd-surface/src/host.rs crates/agentd-surface/src/http.rs crates/agentd-surface/src/test_support.rs crates/agentd-surface/tests/http.rs crates/agentd-bin/src/host.rs
git commit -m "feat(messaging): per-recipient message suppression"
```

---

### Task 6: record M3 Plan B in the parity map with its contract tests

The parity map is the authoritative replacement record and its cells are contract-tested. Statuses stay `partial` — Matrix/remote-relay delivery, notification gates, dashboard message views, cutover, rollback and token provisioning are all still outstanding for these rows — so only the decision text changes, and the contract test lands in the same commit.

**Files:**
- Modify: `docs/parity/agent-chat-capability-map.md` (the `messaging_inbox` and `group_messaging` rows only)
- Test: `crates/agentctl/tests/parity_cli.rs` (append)

**Interfaces:**
- Consumes: `parity_rows() -> Vec<ParityRow>` with `row.capability`, `row.status`, `row.decision` — the existing helper at `crates/agentctl/tests/parity_cli.rs:333`.
- Produces: nothing consumed by later tasks; this is the last task.

- [ ] **Step 1: Write the failing contract test**

Append to `crates/agentctl/tests/parity_cli.rs`:

```rust
#[test]
fn parity_capability_map_records_m3_plan_b_messaging_progress() {
    let rows = parity_rows();
    let messaging = rows
        .iter()
        .find(|row| row.capability == "messaging_inbox")
        .expect("messaging_inbox row");
    let group = rows
        .iter()
        .find(|row| row.capability == "group_messaging")
        .expect("group_messaging row");

    assert_eq!(
        messaging.status, "partial",
        "Matrix/relay delivery, notification gates and dashboards remain open"
    );
    assert_eq!(group.status, "partial");

    for expected in [
        "M3 Plan B",
        "advances the durable read cursor by default",
        "kinds",
        "non-advancing preview",
        "structured `schema`",
        "current members",
        "/api/messages/:id/suppress",
    ] {
        assert!(
            messaging.decision.contains(expected),
            "messaging_inbox decision should mention {expected}: {}",
            messaging.decision
        );
    }
    for expected in [
        "M3 Plan B",
        "current members",
        "/api/messages/:id/suppress",
    ] {
        assert!(
            group.decision.contains(expected),
            "group_messaging decision should mention {expected}: {}",
            group.decision
        );
    }
}
```

- [ ] **Step 2: Run the contract test to verify it fails**

Run: `cargo nextest run -p agentctl --test parity_cli parity_capability_map_records_m3_plan_b_messaging_progress`
Expected: FAIL — the decision cells do not yet mention `M3 Plan B`.

- [ ] **Step 3: Update the `messaging_inbox` decision cell**

In `docs/parity/agent-chat-capability-map.md`, in the `messaging_inbox` row, insert this sentence immediately **before** the closing sentence that begins `This remains partial until remaining attachments/media parity`:

```
M3 Plan B closes the remaining live-path divergences from agent-chat: `check_inbox` advances the durable read cursor by default like agent-chat's `GET /api/inbox/:agent` so a restart or a repeat call never re-delivers consumed mail, an agent-chat-compatible `kinds` filter turns a filtered read into a non-advancing preview (a single cursor cannot advance over one kind without skipping unread messages of every other kind), `send_message` accepts the structured `schema` object the live MCP path previously dropped while `POST /api/messages` and the p224 import path preserved it, group mentions are delivered only to current members of the mentioning group so a removed member stops receiving that group's mail, and `POST /api/messages/:id/suppress` drops one message from one recipient's unread inbox without moving that agent's other read state.
```

Keep it on the same physical line as the rest of the cell — the row is one Markdown table line and a newline would break `parse_rows`.

- [ ] **Step 4: Update the `group_messaging` decision cell**

In the same file, in the `group_messaging` row, insert this sentence immediately **before** the closing sentence that begins `This remains partial until full attachments/media staging`:

```
M3 Plan B scopes group-mention inbox delivery to current members of the mentioning group, so an agent removed through `POST /api/groups/:name/members` stops receiving that group's mentions and imported mentions of non-members are never delivered, and adds per-recipient `POST /api/messages/:id/suppress` so one group mention can be dropped for one member without touching that member's other unread group state.
```

- [ ] **Step 5: Run the full parity contract suite**

Run: `cargo nextest run -p agentctl --test parity_cli`
Expected: PASS — the new test plus every pre-existing p217/p218/p219/p220/p221/p222/p223 assertion, all of which are `contains` checks that additive text cannot break.

Then run: `cargo nextest run -p agentctl --test worktree_reconciliation_contract`
Expected: PASS.

Then run: `cargo nextest run -p agentctl --test enterprise_project_authority_contract`
Expected: PASS.

- [ ] **Step 6: Format, lint, commit**

```bash
cargo fmt --all
cargo clippy -p agentctl --all-targets -- -D warnings
git add docs/parity/agent-chat-capability-map.md crates/agentctl/tests/parity_cli.rs
git commit -m "docs(parity): record M3 Plan B messaging parity evidence"
```

---

## Exit Gate

After Task 6, run the branch gate once (a single workspace `nextest` run is allowed here as the final gate, never as a per-task gate):

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo nextest run
```

Expected: green apart from the known load-sensitive flake `native_runtime_can_terminate_a_running_child` in `agentd-tmux` (untouched by this branch); re-run it in isolation to confirm it passes.

**Behavioural change to carry into the merge release note:** `GET /api/inbox/:agent` and the `check_inbox` MCP tool now consume by default. Any caller that polled the inbox for observation must pass `?drain=false` / `"drain": false`.
