use agentd_store::{SqliteStore, message_repo};
use serde_json::{Value, json};

async fn open_temp() -> (SqliteStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");
    (store, dir)
}

fn text(value: &str) -> String {
    value.to_string()
}

fn direct_message(id: &str) -> message_repo::DirectMessageInput {
    message_repo::DirectMessageInput {
        message_id: Some(text(id)),
        ts: Some(1_780_049_205_450),
        from: text("alex"),
        to: text("codex-worker"),
        message_type: Some(text("human")),
        priority: Some(text("normal")),
        summary: text("please inspect the failing smoke"),
        full: text("Please inspect the failing smoke and report the root cause."),
        reply_to: None,
        source: Some(text("api")),
        source_room: None,
        sender_mxid: None,
        trust_level: Some(text("operator")),
        from_id: Some(text("alex")),
        schema: None,
        attachments: Vec::new(),
    }
}

fn group_message(summary: &str, mentions: &[&str]) -> message_repo::GroupMessageInput {
    message_repo::GroupMessageInput {
        message_id: None,
        ts: None,
        from: text("codex-a"),
        group: text("factory"),
        message_type: Some(text("inform")),
        priority: Some(text("normal")),
        summary: text(summary),
        full: format!("full: {summary}"),
        mentions: mentions.iter().map(|value| text(value)).collect(),
        reply_to: None,
        source: Some(text("api")),
        schema: None,
        attachments: Vec::new(),
    }
}

fn attachment_meta() -> Value {
    json!({
        "path": "/tmp/agentd-p221-note.txt",
        "source_path": "/tmp/agentd-p221-note.txt",
        "name": "note.txt",
        "mime": "text/plain",
        "kind": "file",
        "size": 12,
        "staged": false
    })
}

#[tokio::test]
async fn direct_messages_round_trip_schema_metadata_for_task_graphs() {
    let (store, _dir) = open_temp().await;
    let mut direct = direct_message("msg_task_graph_result");
    direct.schema = Some(json!({
        "kind": "task_graph_result",
        "version": 1,
        "payload": {
            "graphId": "graph_live",
            "nodeId": "a",
            "result": { "ok": true }
        }
    }));

    message_repo::insert_direct_message(store.pool(), direct)
        .await
        .expect("insert direct message with schema");

    let preview = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("preview");
    assert_eq!(preview.len(), 1);
    let schema = preview[0].schema.as_ref().expect("schema preserved");
    assert_eq!(schema["kind"], "task_graph_result");
    assert_eq!(schema["payload"]["graphId"], "graph_live");
    assert_eq!(schema["payload"]["nodeId"], "a");
    assert_eq!(schema["payload"]["result"]["ok"], true);
}

#[tokio::test]
async fn direct_inbox_reads_unread_messages_and_drain_marks_read() {
    let (store, _dir) = open_temp().await;

    message_repo::insert_direct_message(store.pool(), direct_message("msg_direct_1"))
        .await
        .expect("insert direct message");

    let preview = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("preview");
    assert_eq!(preview.len(), 1);
    assert_eq!(preview[0].id, "msg_direct_1");
    assert_eq!(preview[0].from, "alex");
    assert_eq!(preview[0].to, "codex-worker");
    assert_eq!(preview[0].message_type, "human");
    assert_eq!(preview[0].priority, "normal");
    assert_eq!(preview[0].summary, "please inspect the failing smoke");
    assert_eq!(
        preview[0].full,
        "Please inspect the failing smoke and report the root cause."
    );
    assert_eq!(preview[0].source, "api");
    assert_eq!(preview[0].trust_level.as_deref(), Some("operator"));
    assert_eq!(preview[0].ts, 1_780_049_205_450);
    assert!(
        !preview[0].at.is_empty(),
        "agent-facing at field is present"
    );
    assert!(
        !preview[0].time.is_empty(),
        "agent-facing time field is present"
    );

    let preview_again = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("preview again");
    assert_eq!(preview_again.len(), 1, "preview does not mark read");

    let drained = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: true },
    )
    .await
    .expect("drain");
    assert_eq!(drained.len(), 1);
    assert_eq!(drained[0].id, "msg_direct_1");

    let after_drain = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("after drain");
    assert!(after_drain.is_empty(), "drain marks returned rows read");
}

#[tokio::test]
async fn direct_inbox_insert_is_idempotent_by_message_id() {
    let (store, _dir) = open_temp().await;

    let first = message_repo::insert_direct_message(store.pool(), direct_message("msg_direct_2"))
        .await
        .expect("first insert");
    let second = message_repo::insert_direct_message(store.pool(), direct_message("msg_direct_2"))
        .await
        .expect("second insert");
    assert_eq!(first.id, second.id);

    let unread = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("preview");
    assert_eq!(unread.len(), 1, "message_id is an idempotency key");
    assert_eq!(unread[0].id, "msg_direct_2");
}

#[tokio::test]
async fn direct_and_group_messages_round_trip_attachment_metadata() {
    let (store, _dir) = open_temp().await;
    let attachment = attachment_meta();

    let mut direct = direct_message("msg_direct_with_attachment");
    direct.attachments = vec![attachment.clone()];
    message_repo::insert_direct_message(store.pool(), direct)
        .await
        .expect("insert direct message");

    let direct_preview = message_repo::read_direct_inbox(
        store.pool(),
        "codex-worker",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("direct preview");
    assert_eq!(direct_preview.len(), 1);
    assert_eq!(direct_preview[0].attachments, vec![attachment.clone()]);

    message_repo::create_group(
        store.pool(),
        message_repo::GroupCreateInput {
            name: text("factory"),
            members: vec![text("codex-a"), text("codex-b")],
        },
    )
    .await
    .expect("create group");
    let mut group = group_message("with attachment", &["codex-b"]);
    group.attachments = vec![attachment.clone()];
    message_repo::insert_group_message(store.pool(), group)
        .await
        .expect("insert group message");

    let history = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 10,
            unread_limit: None,
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("group history");
    assert_eq!(history.unread.len(), 1);
    assert_eq!(history.unread[0].attachments, vec![attachment]);
}

#[tokio::test]
async fn group_delete_cascades_members_and_messages() {
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
    message_repo::insert_group_message(store.pool(), group_message("delete me", &["codex-b"]))
        .await
        .expect("insert group message");

    let before = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 10,
            unread_limit: None,
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("read before delete");
    assert_eq!(before.unread.len(), 1);

    let deleted = message_repo::delete_group(store.pool(), "factory")
        .await
        .expect("delete group")
        .expect("deleted group");
    assert_eq!(deleted.name, "factory");
    assert_eq!(deleted.members, ["codex-a", "codex-b"]);
    assert!(
        message_repo::get_group(store.pool(), "factory")
            .await
            .expect("get deleted group")
            .is_none()
    );
    assert!(
        message_repo::list_groups(store.pool())
            .await
            .expect("list groups")
            .is_empty()
    );

    let after = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 10,
            unread_limit: None,
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("read after delete");
    assert!(after.unread.is_empty());
    assert!(after.read.is_empty());
    assert_eq!(after.unread_total, 0);
}

#[tokio::test]
async fn group_message_mentions_appear_in_inbox_and_are_drainable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db = dir.path().join("agentd.db");
    let store = SqliteStore::connect(&db).await.expect("connect + migrate");

    message_repo::create_group(
        store.pool(),
        message_repo::GroupCreateInput {
            name: text("factory"),
            members: vec![text("codex-a"), text("codex-b"), text("codex-c")],
        },
    )
    .await
    .expect("create group");
    message_repo::insert_group_message(store.pool(), group_message("mention b", &["codex-b"]))
        .await
        .expect("first group message");
    message_repo::insert_group_message(store.pool(), group_message("no mention", &[]))
        .await
        .expect("second group message");

    let b_preview = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("b preview");
    assert!(b_preview.dm.is_empty());
    assert_eq!(b_preview.group.len(), 1);
    assert_eq!(b_preview.group[0].summary, "mention b");
    assert_eq!(b_preview.group[0].group, "factory");
    assert_eq!(b_preview.group[0].mentions, vec![text("codex-b")]);

    let c_preview = message_repo::read_agent_inbox(
        store.pool(),
        "codex-c",
        message_repo::InboxReadOptions { drain: false },
    )
    .await
    .expect("c preview");
    assert!(c_preview.group.is_empty());

    let b_drain = message_repo::read_agent_inbox(
        store.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: true },
    )
    .await
    .expect("b drain");
    assert_eq!(b_drain.group.len(), 1);
    drop(store);

    let reopened = SqliteStore::connect(&db).await.expect("reopen");
    let after_drain = message_repo::read_agent_inbox(
        reopened.pool(),
        "codex-b",
        message_repo::InboxReadOptions { drain: true },
    )
    .await
    .expect("after drain");
    assert!(after_drain.group.is_empty(), "group mention drain persists");
}

#[tokio::test]
async fn group_messages_preview_and_read_all_advances() {
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
    for summary in ["one", "two", "three"] {
        message_repo::insert_group_message(store.pool(), group_message(summary, &["codex-b"]))
            .await
            .expect("group message");
    }

    let preview = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 1,
            unread_limit: Some(2),
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("preview");
    assert_eq!(preview.group, "factory");
    assert_eq!(preview.unread_total, 3);
    assert_eq!(preview.unread_returned, 2);
    assert_eq!(preview.unread_omitted, 1);
    assert_eq!(
        preview
            .unread
            .iter()
            .map(|message| message.summary.as_str())
            .collect::<Vec<_>>(),
        ["one", "two"]
    );
    assert_eq!(preview.advance, message_repo::GroupReadAdvance::None);

    let preview_again = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 1,
            unread_limit: Some(2),
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("preview again");
    assert_eq!(preview_again.unread_total, 3);

    let consumed = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 10,
            unread_limit: None,
            advance: message_repo::GroupReadAdvance::All,
        },
    )
    .await
    .expect("consume all");
    assert_eq!(consumed.unread_total, 3);
    assert_eq!(consumed.advance, message_repo::GroupReadAdvance::All);

    let after = message_repo::read_group_messages(
        store.pool(),
        "factory",
        "codex-b",
        message_repo::GroupReadOptions {
            limit: 10,
            unread_limit: Some(10),
            advance: message_repo::GroupReadAdvance::None,
        },
    )
    .await
    .expect("after consume");
    assert_eq!(after.unread_total, 0);
    assert_eq!(after.read.len(), 3);
}
