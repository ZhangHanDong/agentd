use agentd_store::{SqliteStore, matrix_bridge_repo};

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");
    (store, dir)
}

fn text(value: &str) -> String {
    value.to_string()
}

/// Accept one open command in `room_id` and run the dispatch sweep, leaving it
/// `running` against a freshly created graph — the state every settle test
/// starts from.
async fn running_command(
    store: &SqliteStore,
    room_id: &str,
    event_id: &str,
    body: &str,
) -> matrix_bridge_repo::MatrixCommandRecord {
    matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text(room_id),
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
                event_id: text(event_id),
                room_id: text(room_id),
                project_id: None,
                sender_mxid: text("@alice:matrix.test"),
                route: text("agent"),
                body: text(body),
                open: true,
                run_request: Some(matrix_bridge_repo::MatrixCommandRunPlan {
                    label: text("build"),
                    owner: text("alice"),
                    assignee: text("codex-worker"),
                    description: text(body),
                }),
            },
            direct: None,
            group: None,
            relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
        },
    )
    .await
    .expect("accept command");

    agentd_store::matrix_command_dispatch::dispatch_accepted_commands(store.pool())
        .await
        .expect("dispatch");

    let command = matrix_bridge_repo::get_command(store.pool(), &accepted.command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(command.status, "running");
    command
}

/// Drive the single `run` node of a dispatched command's graph to `status`,
/// which settles the graph itself.
async fn finish_run_node(store: &SqliteStore, graph_id: &str, status: &str) {
    let (graph, _node) = agentd_store::agent_chat_task_graph_repo::update_node_and_advance(
        store.pool(),
        graph_id,
        "run",
        agentd_store::agent_chat_task_graph_repo::UpdateAgentChatTaskGraphNode {
            status: Some(text(status)),
            result: Some(serde_json::json!({ "ok": status == "complete" })),
            error: None,
        },
    )
    .await
    .expect("update run node")
    .expect("graph exists");
    assert_eq!(graph.status, status);
}

#[tokio::test]
async fn matrix_bridge_store_persists_room_mapping_and_event_records() {
    let (store, _dir) = open_temp().await;

    let room = matrix_bridge_repo::upsert_room(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeRoomInput {
            room_id: text("!ops:matrix.test"),
            project_id: Some(text("project-ops")),
            group_name: Some(text("ops")),
            agent_name: None,
            trusted: true,
            trust_reason: text("managed"),
            inviter_mxid: Some(text("@alice:matrix.test")),
        },
    )
    .await
    .expect("upsert room");

    assert_eq!(room.room_id, "!ops:matrix.test");
    assert_eq!(room.project_id.as_deref(), Some("project-ops"));
    assert_eq!(room.group_name.as_deref(), Some("ops"));
    assert!(room.trusted);
    assert_eq!(room.trust_reason, "managed");

    let loaded = matrix_bridge_repo::get_room(store.pool(), "!ops:matrix.test")
        .await
        .expect("get room")
        .expect("room exists");
    assert_eq!(loaded.group_name.as_deref(), Some("ops"));
    assert_eq!(loaded.inviter_mxid.as_deref(), Some("@alice:matrix.test"));

    let event = matrix_bridge_repo::record_event(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeEventInput {
            event_id: text("$event-1"),
            room_id: text("!ops:matrix.test"),
            sender_mxid: text("@alice:matrix.test"),
            message_id: Some(text("msg-1")),
            route: text("group"),
            ignored: false,
        },
    )
    .await
    .expect("record event");
    assert_eq!(event.event_id, "$event-1");
    assert_eq!(event.message_id.as_deref(), Some("msg-1"));

    let duplicate = matrix_bridge_repo::record_event(
        store.pool(),
        matrix_bridge_repo::MatrixBridgeEventInput {
            event_id: text("$event-1"),
            room_id: text("!ops:matrix.test"),
            sender_mxid: text("@alice:matrix.test"),
            message_id: Some(text("msg-duplicate")),
            route: text("group"),
            ignored: false,
        },
    )
    .await
    .expect("record duplicate event");
    assert_eq!(duplicate.message_id.as_deref(), Some("msg-1"));
}

#[tokio::test]
async fn matrix_outbox_cursor_is_durable_and_monotonic() {
    let (store, _dir) = open_temp().await;
    assert_eq!(
        matrix_bridge_repo::get_outbox_cursor(store.pool(), "matrix-bridge")
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        matrix_bridge_repo::acknowledge_outbox_cursor(store.pool(), "matrix-bridge", 12)
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        matrix_bridge_repo::acknowledge_outbox_cursor(store.pool(), "matrix-bridge", 7)
            .await
            .unwrap(),
        12
    );
    assert_eq!(
        matrix_bridge_repo::get_outbox_cursor(store.pool(), "matrix-bridge")
            .await
            .unwrap(),
        12
    );
}

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

#[tokio::test]
async fn matrix_command_id_is_canonical_and_deterministic() {
    let first = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-1");
    let again = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-1");
    let other_event = matrix_bridge_repo::matrix_command_id("!ops:matrix.test", "$event-2");
    let other_room = matrix_bridge_repo::matrix_command_id("!other:matrix.test", "$event-1");

    assert_eq!(
        first, again,
        "the same event always yields the same command id"
    );
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

    // The rejected event must leave nothing behind: no half-written event row.
    let events: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matrix_bridge_events")
        .fetch_one(store.pool())
        .await
        .expect("count events");
    assert_eq!(events, 1, "a rolled-back acceptance writes no event row");

    // A plain (non-open) command with the same payload does not contend.
    let mut chat = open_command("$chat-1");
    chat.command.open = false;
    chat.command.run_request = None;
    matrix_bridge_repo::accept_inbound_event(store.pool(), chat)
        .await
        .expect("plain chat is never blocked by the open-dedup slot");
}

#[tokio::test]
async fn accept_inbound_event_rejects_an_open_flag_that_disagrees_with_the_run_request() {
    let (store, _dir) = open_temp().await;

    // `open` is what the partial unique index keys on, `run_request` is what
    // the sweep acts on. A caller that computes them inconsistently would
    // either disable dedup or enqueue a run nobody holds a slot for, so this
    // is a hard invariant rather than a silent coercion.
    let mismatch = matrix_bridge_repo::accept_inbound_event(
        store.pool(),
        matrix_bridge_repo::MatrixInboundAcceptance {
            command: matrix_bridge_repo::MatrixCommandInput {
                event_id: text("$bad-1"),
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
    .expect_err("open without a run request must be rejected");
    assert!(mismatch.to_string().contains("run request"), "{mismatch}");

    let commands: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matrix_commands")
        .fetch_one(store.pool())
        .await
        .expect("count commands");
    assert_eq!(commands, 0);
}

#[tokio::test]
async fn accept_inbound_event_repairs_a_torn_write_instead_of_duplicating_the_message() {
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

    // Simulate the pre-M4 crash window: the inbox message landed but the
    // acceptance record never did. The deterministic id is the whole point —
    // the replay must adopt this row, not mint a second one.
    let command_id = matrix_bridge_repo::matrix_command_id("!dm:matrix.test", "$dm-torn");
    let message_id = format!("msg_{command_id}");
    agentd_store::message_repo::insert_direct_message(
        store.pool(),
        agentd_store::message_repo::DirectMessageInput {
            message_id: Some(message_id.clone()),
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
        },
    )
    .await
    .expect("orphaned message from the crash window");

    let recovered = matrix_bridge_repo::accept_inbound_event(
        store.pool(),
        matrix_bridge_repo::MatrixInboundAcceptance {
            command: matrix_bridge_repo::MatrixCommandInput {
                event_id: text("$dm-torn"),
                room_id: text("!dm:matrix.test"),
                project_id: None,
                sender_mxid: text("@alice:matrix.test"),
                route: text("agent"),
                body: text("please review the patch"),
                open: false,
                run_request: None,
            },
            direct: Some(agentd_store::message_repo::DirectMessageInput {
                message_id: Some(message_id.clone()),
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
        },
    )
    .await
    .expect("replay after a torn write");

    assert!(!recovered.duplicate);
    assert_eq!(
        recovered.direct.expect("direct message").id,
        message_id,
        "the replay adopts the orphaned message"
    );
    assert_eq!(
        recovered.command.message_id.as_deref(),
        Some(message_id.as_str())
    );

    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM direct_messages")
        .fetch_one(store.pool())
        .await
        .expect("count messages");
    assert_eq!(
        messages, 1,
        "a torn write must not leave two inbox messages"
    );
}

#[tokio::test]
async fn accept_inbound_event_serializes_concurrent_posts_of_one_event() {
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

    let command_id = matrix_bridge_repo::matrix_command_id("!dm:matrix.test", "$dm-race");
    let acceptance = || matrix_bridge_repo::MatrixInboundAcceptance {
        command: matrix_bridge_repo::MatrixCommandInput {
            event_id: text("$dm-race"),
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

    // Two POSTs of one event id, in flight together. Before the transaction
    // both passed the read-only duplicate check and both wrote a message.
    let (left, right) = tokio::join!(
        matrix_bridge_repo::accept_inbound_event(store.pool(), acceptance()),
        matrix_bridge_repo::accept_inbound_event(store.pool(), acceptance()),
    );
    let left = left.expect("first concurrent acceptance");
    let right = right.expect("second concurrent acceptance");
    assert_eq!(left.command.command_id, command_id);
    assert_eq!(right.command.command_id, command_id);
    assert!(
        left.duplicate ^ right.duplicate,
        "exactly one of the two racing posts creates the command"
    );

    let messages: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM direct_messages")
        .fetch_one(store.pool())
        .await
        .expect("count messages");
    assert_eq!(messages, 1);
    let commands: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM matrix_commands")
        .fetch_one(store.pool())
        .await
        .expect("count commands");
    assert_eq!(commands, 1);
    let outbox: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM relay_stream_events")
        .fetch_one(store.pool())
        .await
        .expect("count outbox");
    assert_eq!(outbox, 1);
}

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
        Some(matrix_bridge_repo::matrix_command_graph_id(&accepted.command.command_id).as_str())
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

#[tokio::test]
async fn dispatch_adopts_a_graph_left_behind_by_a_crash_before_the_bind() {
    // The real crash window is between `create_graph` and `bind_command_run`:
    // the graph is durable, the command is still `accepted` with no `run_id`,
    // and the next tick re-lists it. Simulated here by creating the graph at
    // the deterministic id out of band, exactly as the crashed sweep left it.
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
                event_id: text("$run-crash"),
                room_id: text("!ops:matrix.test"),
                project_id: None,
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

    let graph_id = matrix_bridge_repo::matrix_command_graph_id(&accepted.command.command_id);
    let mut nodes = std::collections::BTreeMap::new();
    nodes.insert(
        "run".to_string(),
        agentd_store::agent_chat_task_graph_repo::AgentChatTaskGraphNodeInput {
            id: None,
            assignee: Some(text("codex-worker")),
            role: None,
            capability: None,
            description: text("run the build"),
            depends_on: Vec::new(),
            condition: None,
            execution: None,
        },
    );
    agentd_store::agent_chat_task_graph_repo::create_graph(
        store.pool(),
        agentd_store::agent_chat_task_graph_repo::CreateAgentChatTaskGraph {
            id: Some(graph_id.clone()),
            owner: text("alice"),
            label: text("build"),
            nodes,
        },
    )
    .await
    .expect("pre-existing graph from the crashed sweep");

    let dispatched =
        agentd_store::matrix_command_dispatch::dispatch_accepted_commands(store.pool())
            .await
            .expect("dispatch after crash");
    assert_eq!(
        dispatched, 1,
        "the orphaned graph is adopted, not abandoned"
    );

    let graphs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat_task_graphs")
        .fetch_one(store.pool())
        .await
        .expect("count graphs");
    assert_eq!(
        graphs, 1,
        "the crashed sweep's graph is reused, not duplicated"
    );

    let command = matrix_bridge_repo::get_command(store.pool(), &accepted.command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(command.status, "running");
    assert_eq!(command.run_id.as_deref(), Some(graph_id.as_str()));
}

#[tokio::test]
async fn dispatch_settles_an_accepted_command_that_carries_no_run_plan() {
    // Plain chat is written `settled` by the inbound writer, so an `accepted`
    // row with no plan can only come from an older schema or a torn write. It
    // must not sit in the open-dedup slot forever.
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
                event_id: text("$chat-1"),
                room_id: text("!ops:matrix.test"),
                project_id: None,
                sender_mxid: text("@alice:matrix.test"),
                route: text("agent"),
                body: text("just chatting"),
                open: false,
                run_request: None,
            },
            direct: None,
            group: None,
            relay_payload: serde_json::json!({ "kind": "direct", "source": "matrix" }),
        },
    )
    .await
    .expect("accept command");
    assert_eq!(accepted.command.status, "settled");

    sqlx::query("UPDATE matrix_commands SET status = 'accepted' WHERE command_id = ?")
        .bind(&accepted.command.command_id)
        .execute(store.pool())
        .await
        .expect("force the stray accepted row");

    let dispatched =
        agentd_store::matrix_command_dispatch::dispatch_accepted_commands(store.pool())
            .await
            .expect("dispatch");
    assert_eq!(dispatched, 0, "a command with no run plan creates no run");

    let command = matrix_bridge_repo::get_command(store.pool(), &accepted.command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(command.status, "settled");
    assert!(command.run_id.is_none());
    let graphs: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM agent_chat_task_graphs")
        .fetch_one(store.pool())
        .await
        .expect("count graphs");
    assert_eq!(graphs, 0);
}

#[tokio::test]
async fn settling_a_finished_command_lets_the_same_text_be_sent_again() {
    // The user-visible bug: the open-dedup index covers `accepted` and
    // `running`, so a command left at `running` after its run finished held
    // the room's slot forever and made "run the build" un-repeatable there.
    let (store, _dir) = open_temp().await;
    let command = running_command(&store, "!ops:matrix.test", "$run-1", "run the build").await;
    let graph_id = command.run_id.clone().expect("bound run");

    finish_run_node(&store, &graph_id, "complete").await;

    let settled = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
        .await
        .expect("settle sweep");
    assert_eq!(settled, 1);

    let after = matrix_bridge_repo::get_command(store.pool(), &command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(after.status, "settled");
    assert_eq!(after.record_version, command.record_version + 1);

    // The point of settling: the identical command text is accepted again.
    let resent = matrix_bridge_repo::accept_inbound_event(
        store.pool(),
        matrix_bridge_repo::MatrixInboundAcceptance {
            command: matrix_bridge_repo::MatrixCommandInput {
                event_id: text("$run-2"),
                room_id: text("!ops:matrix.test"),
                project_id: None,
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
    .expect("re-sending a finished command must be accepted, not rejected as open");
    assert_eq!(resent.command.status, "accepted");
    assert!(!resent.duplicate);
    assert_eq!(resent.command.dedup_key, command.dedup_key);
}

#[tokio::test]
async fn settling_covers_failed_and_cancelled_runs_too() {
    let (store, _dir) = open_temp().await;
    let failed = running_command(
        &store,
        "!failed:matrix.test",
        "$run-failed",
        "run the build",
    )
    .await;
    let cancelled = running_command(
        &store,
        "!cancelled:matrix.test",
        "$run-cancelled",
        "run the build",
    )
    .await;

    finish_run_node(&store, &failed.run_id.clone().expect("bound run"), "failed").await;
    let cancelled_graph = cancelled.run_id.clone().expect("bound run");
    let deleted =
        agentd_store::agent_chat_task_graph_repo::delete_graph(store.pool(), &cancelled_graph)
            .await
            .expect("cancel graph")
            .expect("graph exists");
    assert_eq!(deleted.status, "cancelled");

    let settled = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
        .await
        .expect("settle sweep");
    assert_eq!(settled, 2, "a cancelled run must release its slot too");

    for command_id in [&failed.command_id, &cancelled.command_id] {
        let after = matrix_bridge_repo::get_command(store.pool(), command_id)
            .await
            .expect("get command")
            .expect("command exists");
        assert_eq!(after.status, "settled", "{command_id}");
    }
}

#[tokio::test]
async fn settling_leaves_a_command_whose_run_is_still_active() {
    let (store, _dir) = open_temp().await;
    let command = running_command(&store, "!ops:matrix.test", "$run-1", "run the build").await;

    let settled = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
        .await
        .expect("settle sweep");
    assert_eq!(settled, 0, "an active run still owns its command");

    let after = matrix_bridge_repo::get_command(store.pool(), &command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(after.status, "running");
    assert_eq!(after.record_version, command.record_version);
}

#[tokio::test]
async fn settling_releases_a_command_whose_graph_row_is_gone() {
    // The last way the slot can leak: the graph row was removed out of band,
    // so no terminal status will ever be observable for it.
    let (store, _dir) = open_temp().await;
    let command = running_command(&store, "!ops:matrix.test", "$run-1", "run the build").await;

    sqlx::query("DELETE FROM agent_chat_task_graphs WHERE id = ?")
        .bind(command.run_id.as_deref().expect("bound run"))
        .execute(store.pool())
        .await
        .expect("orphan the run");

    let settled = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
        .await
        .expect("settle sweep");
    assert_eq!(settled, 1);

    let after = matrix_bridge_repo::get_command(store.pool(), &command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(after.status, "settled");
}

#[tokio::test]
async fn a_replayed_settle_sweep_is_a_no_op() {
    let (store, _dir) = open_temp().await;
    let command = running_command(&store, "!ops:matrix.test", "$run-1", "run the build").await;
    finish_run_node(
        &store,
        &command.run_id.clone().expect("bound run"),
        "complete",
    )
    .await;

    let first = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
        .await
        .expect("settle sweep");
    assert_eq!(first, 1);
    let settled = matrix_bridge_repo::get_command(store.pool(), &command.command_id)
        .await
        .expect("get command")
        .expect("command exists");

    for _ in 0..3 {
        let again = agentd_store::matrix_command_dispatch::settle_running_commands(store.pool())
            .await
            .expect("replayed settle sweep must not error");
        assert_eq!(again, 0);
    }

    let after = matrix_bridge_repo::get_command(store.pool(), &command.command_id)
        .await
        .expect("get command")
        .expect("command exists");
    assert_eq!(after.status, "settled");
    assert_eq!(
        after.record_version, settled.record_version,
        "a replayed settle must not bump the version"
    );
}
