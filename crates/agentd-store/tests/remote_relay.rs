use agentd_store::{SqliteStore, relay_repo};
use serde_json::json;

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
async fn remote_relay_store_persists_server_heartbeats_and_delivery_events() {
    let (store, _dir) = open_temp().await;

    let server = relay_repo::record_server_heartbeat(
        store.pool(),
        relay_repo::ServerHeartbeatInput {
            server: text("remote-host-1"),
            instance_id: Some(text("inst-abc")),
            boot_ts: Some(1_780_100_001),
            agents: vec![text("codex-a"), text("codex-b")],
            sessions: vec![text("codex-a:0.0"), text("codex-b:0.0")],
        },
    )
    .await
    .expect("record heartbeat");

    assert_eq!(server.id, "remote-host-1");
    assert!(server.online);
    assert_eq!(server.agent_count, 2);
    assert_eq!(server.sessions, vec!["codex-a:0.0", "codex-b:0.0"]);

    let event = relay_repo::append_delivery_event(
        store.pool(),
        relay_repo::DeliveryEventInput {
            event_type: text("relay.delivered"),
            message_id: Some(text("msg-1")),
            queue_entry_id: None,
            agent: Some(text("codex-a")),
            target: Some(text("codex-a:0.0")),
            reason: None,
            source: Some(text("push-relay")),
            context: json!({
                "server": "remote-host-1",
                "relayInstanceId": "inst-abc"
            }),
        },
    )
    .await
    .expect("append delivery event");

    assert_eq!(event.event_type, "relay.delivered");
    assert_eq!(event.agent.as_deref(), Some("codex-a"));

    let listed = relay_repo::list_delivery_events_for_agent(store.pool(), "codex-a", 10)
        .await
        .expect("list delivery events");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].message_id.as_deref(), Some("msg-1"));
    assert_eq!(listed[0].context["server"], "remote-host-1");
    assert_eq!(listed[0].context["relayInstanceId"], "inst-abc");
}
