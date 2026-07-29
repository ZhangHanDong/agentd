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

#[derive(Debug, Default)]
struct CursorBackend {
    seeded: Option<(Option<String>, i64)>,
    advances: Vec<(Option<String>, Option<String>, Option<i64>)>,
    next_version: i64,
    inbound: Vec<MatrixInboundEvent>,
    fail_inbound: Option<String>,
}

impl AgentdBridgeBackend for CursorBackend {
    fn register_room(&mut self, _room: MatrixRoomRegistration) -> Result<(), BridgeError> {
        Ok(())
    }

    fn post_inbound(&mut self, event: MatrixInboundEvent) -> Result<(), BridgeError> {
        if self.fail_inbound.as_deref() == Some(event.event_id.as_str()) {
            return Err(BridgeError::backend(format!("rejected {}", event.event_id)));
        }
        self.inbound.push(event);
        Ok(())
    }

    fn poll_outbox(&mut self, _from_seq: i64) -> Result<Vec<MatrixOutboundEvent>, BridgeError> {
        Ok(Vec::new())
    }

    fn gateway_cursor(&mut self) -> Result<Option<(Option<String>, i64)>, BridgeError> {
        Ok(self.seeded.clone())
    }

    fn advance_gateway_cursor(
        &mut self,
        sync_token: Option<&str>,
        last_event_id: Option<&str>,
        expected_version: Option<i64>,
    ) -> Result<i64, BridgeError> {
        self.advances.push((
            sync_token.map(ToOwned::to_owned),
            last_event_id.map(ToOwned::to_owned),
            expected_version,
        ));
        self.next_version += 1;
        Ok(self.next_version)
    }
}

#[test]
fn matrix_bridge_runtime_seeds_from_the_daemon_cursor_and_advances_after_the_batch() {
    let backend = CursorBackend {
        // The daemon, not the local state file, is the authority on where a
        // restarted gateway resumes.
        seeded: Some((Some("s_daemon".to_owned()), 7)),
        next_version: 7,
        ..CursorBackend::default()
    };
    let transport = FakeTransport {
        inbound: vec![
            inbound_event("$event-1", "first"),
            inbound_event("$event-2", "second"),
        ],
        ..FakeTransport::default()
    };
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::new(3));

    let report = runtime.run_once().expect("run once succeeds");

    assert_eq!(report.inbound_forwarded, 2);
    assert_eq!(runtime.state().sync_token(), Some("s_daemon"));
    // One advance for the whole batch, carrying the last event id and the
    // version the seed observed.
    assert_eq!(
        runtime.backend().advances,
        vec![(
            Some("s_daemon".to_owned()),
            Some("$event-2".to_owned()),
            Some(7)
        )]
    );
    assert_eq!(runtime.state().cursor_version(), Some(8));
}

#[test]
fn matrix_bridge_runtime_skips_the_advance_when_no_inbound_events_arrived() {
    let backend = CursorBackend {
        seeded: Some((Some("s_daemon".to_owned()), 7)),
        next_version: 7,
        ..CursorBackend::default()
    };
    let mut runtime = BridgeRuntime::new(backend, FakeTransport::default(), BridgeState::default());

    runtime.run_once().expect("run once succeeds");

    assert!(runtime.backend().advances.is_empty());
    assert_eq!(runtime.state().cursor_version(), Some(7));
}

#[test]
fn matrix_bridge_runtime_does_not_advance_past_a_batch_it_failed_to_deliver() {
    let backend = CursorBackend {
        seeded: Some((Some("s_daemon".to_owned()), 7)),
        next_version: 7,
        fail_inbound: Some("$event-2".to_owned()),
        ..CursorBackend::default()
    };
    let transport = FakeTransport {
        inbound: vec![
            inbound_event("$event-1", "first"),
            inbound_event("$event-2", "second"),
        ],
        ..FakeTransport::default()
    };
    let mut runtime = BridgeRuntime::new(backend, transport, BridgeState::default());

    let err = runtime.run_once().expect_err("second inbound post fails");

    assert_eq!(err.to_string(), "agentd backend error: rejected $event-2");
    // The cursor stayed put, so the next iteration replays the batch rather
    // than skipping its undelivered tail.
    assert!(runtime.backend().advances.is_empty());
    assert_eq!(runtime.state().cursor_version(), Some(7));
}

#[test]
fn bridge_state_defaults_the_gateway_cursor_fields_to_none() {
    let state = BridgeState::new(9);
    assert_eq!(state.next_from_seq(), 9);
    assert_eq!(state.sync_token(), None);
    assert_eq!(state.cursor_version(), None);
}
