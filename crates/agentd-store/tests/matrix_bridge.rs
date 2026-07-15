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
