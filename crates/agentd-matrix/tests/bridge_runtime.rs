use agentd_matrix::{
    AgentdBridgeBackend, BridgeConfig, BridgeError, BridgeRuntime, BridgeState,
    MatrixBridgeTransport, MatrixInboundEvent, MatrixOutboundEvent, MatrixRoomRegistration,
};
use serde_json::json;

#[derive(Debug, Default)]
struct FakeBackend {
    calls: Vec<String>,
    registrations: Vec<MatrixRoomRegistration>,
    inbound: Vec<MatrixInboundEvent>,
    outbox: Vec<MatrixOutboundEvent>,
    polled_from: Vec<i64>,
}

impl AgentdBridgeBackend for FakeBackend {
    fn register_room(&mut self, room: MatrixRoomRegistration) -> Result<(), BridgeError> {
        self.calls.push(format!("room:{}", room.room_id));
        self.registrations.push(room);
        Ok(())
    }

    fn post_inbound(&mut self, event: MatrixInboundEvent) -> Result<(), BridgeError> {
        self.calls.push(format!("inbound:{}", event.event_id));
        self.inbound.push(event);
        Ok(())
    }

    fn poll_outbox(&mut self, from_seq: i64) -> Result<Vec<MatrixOutboundEvent>, BridgeError> {
        self.polled_from.push(from_seq);
        Ok(self.outbox.clone())
    }
}

#[derive(Debug, Default)]
struct FakeTransport {
    rooms: Vec<MatrixRoomRegistration>,
    inbound: Vec<MatrixInboundEvent>,
    sent: Vec<MatrixOutboundEvent>,
    fail_on_seq: Option<i64>,
}

impl MatrixBridgeTransport for FakeTransport {
    fn room_registrations(&mut self) -> Result<Vec<MatrixRoomRegistration>, BridgeError> {
        Ok(self.rooms.clone())
    }

    fn inbound_events(&mut self) -> Result<Vec<MatrixInboundEvent>, BridgeError> {
        Ok(self.inbound.clone())
    }

    fn send_outbound(&mut self, event: MatrixOutboundEvent) -> Result<(), BridgeError> {
        if self.fail_on_seq == Some(event.seq) {
            return Err(BridgeError::transport(format!("failed seq {}", event.seq)));
        }
        self.sent.push(event);
        Ok(())
    }
}

fn group_room() -> MatrixRoomRegistration {
    MatrixRoomRegistration {
        room_id: "!ops:matrix.test".to_owned(),
        group_name: Some("ops".to_owned()),
        agent_name: None,
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()],
    }
}

fn inbound_event(event_id: &str, body: &str) -> MatrixInboundEvent {
    MatrixInboundEvent {
        event_id: event_id.to_owned(),
        room_id: "!ops:matrix.test".to_owned(),
        sender_mxid: "@alex:matrix.test".to_owned(),
        body: body.to_owned(),
        mentions: vec!["codex-worker".to_owned()],
        reply_to: None,
    }
}

fn outbound_event(seq: i64, body: &str) -> MatrixOutboundEvent {
    MatrixOutboundEvent {
        seq,
        room_id: Some("!ops:matrix.test".to_owned()),
        target: Some("codex-worker".to_owned()),
        body: body.to_owned(),
        message_id: Some(format!("msg-{seq}")),
        source: Some("api".to_owned()),
        payload: json!({
            "messageId": format!("msg-{seq}"),
            "source": "api",
            "target": "codex-worker",
            "roomId": "!ops:matrix.test",
            "full": body
        }),
    }
}

#[test]
fn matrix_bridge_runtime_forwards_room_registrations_and_inbound_events() {
    let backend = FakeBackend::default();
    let transport = FakeTransport {
        rooms: vec![group_room()],
        inbound: vec![
            inbound_event("$event-1", "first"),
            inbound_event("$event-2", "second"),
        ],
        ..FakeTransport::default()
    };
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::default());

    let report = runtime.run_once().expect("run once succeeds");

    assert_eq!(
        runtime.backend().calls,
        vec![
            "room:!ops:matrix.test",
            "inbound:$event-1",
            "inbound:$event-2"
        ]
    );
    assert_eq!(runtime.backend().registrations, vec![group_room()]);
    assert_eq!(
        runtime.backend().inbound,
        vec![
            inbound_event("$event-1", "first"),
            inbound_event("$event-2", "second")
        ]
    );
    assert_eq!(report.registered_rooms, 1);
    assert_eq!(report.inbound_forwarded, 2);
}

#[test]
fn matrix_bridge_runtime_sends_outbox_events_and_advances_cursor() {
    let backend = FakeBackend {
        outbox: vec![outbound_event(1, "first"), outbound_event(2, "second")],
        ..FakeBackend::default()
    };
    let transport = FakeTransport::default();
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::default());

    let report = runtime.run_once().expect("run once succeeds");

    assert_eq!(runtime.backend().polled_from, vec![0]);
    assert_eq!(
        runtime.transport().sent,
        vec![outbound_event(1, "first"), outbound_event(2, "second")]
    );
    assert_eq!(runtime.state().next_from_seq(), 2);
    assert_eq!(report.outbound_sent, 2);
}

#[test]
fn matrix_bridge_runtime_keeps_retry_cursor_on_send_failure() {
    let backend = FakeBackend {
        outbox: vec![outbound_event(1, "first"), outbound_event(2, "second")],
        ..FakeBackend::default()
    };
    let transport = FakeTransport {
        fail_on_seq: Some(2),
        ..FakeTransport::default()
    };
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::default());

    let err = runtime.run_once().expect_err("second send fails");

    assert_eq!(err.to_string(), "matrix transport error: failed seq 2");
    assert_eq!(runtime.state().next_from_seq(), 1);
    assert_eq!(runtime.transport().sent, vec![outbound_event(1, "first")]);
}

#[test]
fn matrix_bridge_config_validates_agentd_api_and_defaults_cursor() {
    let config = BridgeConfig::new("http://127.0.0.1:7722///")
        .expect("valid config")
        .with_operator_token("secret-token");

    assert_eq!(config.agentd_api(), "http://127.0.0.1:7722");
    assert_eq!(config.operator_token(), Some("secret-token"));
    assert_eq!(BridgeState::default().next_from_seq(), 0);

    let err = BridgeConfig::new("   ").expect_err("empty url rejected");
    assert_eq!(
        err.to_string(),
        "invalid bridge config: agentd_api is required"
    );
}
