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
