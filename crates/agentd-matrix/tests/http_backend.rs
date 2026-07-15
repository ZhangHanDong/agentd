use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    AgentdBridgeBackend, AgentdHttpBackend, BridgeConfig, BridgeRuntime, BridgeState,
    MatrixBotCommandBackendEffectPort, MatrixBridgeTransport, MatrixInboundEvent,
    MatrixOutboundEvent, MatrixRoomRegistration,
};
use serde_json::json;

#[derive(Debug, Clone)]
struct CapturedRequest {
    method: String,
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl CapturedRequest {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

#[derive(Debug, Clone)]
struct FakeResponse {
    status: u16,
    body: String,
}

impl FakeResponse {
    fn status(status: u16, body: impl Into<String>) -> Self {
        Self {
            status,
            body: body.into(),
        }
    }
}

#[derive(Debug)]
struct FakeAgentdServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: thread::JoinHandle<()>,
}

impl FakeAgentdServer {
    fn new(responses: Vec<FakeResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let request = read_request(&mut stream);
                thread_requests.lock().expect("requests lock").push(request);
                write_response(&mut stream, &response);
            }
        });

        Self {
            base_url,
            requests,
            handle,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("requests lock").clone()
    }

    fn join(self) {
        self.handle.join().expect("fake server thread");
    }
}

fn read_request(stream: &mut std::net::TcpStream) -> CapturedRequest {
    let mut first_line = String::new();
    let mut headers = Vec::new();
    let body;
    let content_length;

    {
        let mut reader = BufReader::new(stream);
        reader.read_line(&mut first_line).expect("request line");

        loop {
            let mut line = String::new();
            reader.read_line(&mut line).expect("header line");
            let trimmed = line.trim_end_matches(['\r', '\n']);
            if trimmed.is_empty() {
                break;
            }
            if let Some((key, value)) = trimmed.split_once(':') {
                headers.push((key.trim().to_owned(), value.trim().to_owned()));
            }
        }

        content_length = headers
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.parse::<usize>().ok())
            .unwrap_or(0);

        let mut body_bytes = vec![0; content_length];
        reader.read_exact(&mut body_bytes).expect("request body");
        body = String::from_utf8(body_bytes).expect("utf8 body");
    }

    let mut parts = first_line.split_whitespace();
    CapturedRequest {
        method: parts.next().expect("method").to_owned(),
        path: parts.next().expect("path").to_owned(),
        headers,
        body,
    }
}

fn write_response(stream: &mut std::net::TcpStream, response: &FakeResponse) {
    let reason = if response.status < 400 { "OK" } else { "ERROR" };
    let bytes = response.body.as_bytes();
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        reason,
        bytes.len()
    );
    stream
        .write_all(header.as_bytes())
        .expect("response header");
    stream.write_all(bytes).expect("response body");
}

fn group_room() -> MatrixRoomRegistration {
    MatrixRoomRegistration {
        room_id: "!ops:matrix.test".to_owned(),
        group_name: Some("ops".to_owned()),
        agent_name: None,
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned()],
    }
}

fn inbound_event() -> MatrixInboundEvent {
    MatrixInboundEvent {
        event_id: "$event-1".to_owned(),
        room_id: "!ops:matrix.test".to_owned(),
        sender_mxid: "@alex:matrix.test".to_owned(),
        body: "please review".to_owned(),
        mentions: vec!["codex-worker".to_owned()],
        reply_to: Some("msg-parent".to_owned()),
    }
}

#[test]
fn agentd_http_backend_posts_room_and_inbound_with_bearer() {
    let server = FakeAgentdServer::new(vec![
        FakeResponse::status(
            200,
            json!({"ok": true, "room": {"roomId": "!ops:matrix.test"}}).to_string(),
        ),
        FakeResponse::status(201, json!({"ok": true, "eventId": "$event-1"}).to_string()),
    ]);
    let config = BridgeConfig::new(server.base_url())
        .expect("config")
        .with_operator_token("bridge-secret");
    let mut backend = AgentdHttpBackend::new(&config).expect("http backend");

    backend.register_room(group_room()).expect("register room");
    backend.post_inbound(inbound_event()).expect("post inbound");

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/matrix/rooms");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    let room_body: serde_json::Value = serde_json::from_str(&requests[0].body).expect("room body");
    assert_eq!(room_body["roomId"], "!ops:matrix.test");
    assert_eq!(room_body["group"], "ops");
    assert_eq!(room_body["trustReason"], "managed");
    assert_eq!(room_body["inviterMxid"], "@alex:matrix.test");
    assert_eq!(room_body["members"], json!(["codex-worker"]));

    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/api/matrix/inbound");
    assert_eq!(
        requests[1].header("authorization"),
        Some("Bearer bridge-secret")
    );
    let inbound_body: serde_json::Value =
        serde_json::from_str(&requests[1].body).expect("inbound body");
    assert_eq!(inbound_body["eventId"], "$event-1");
    assert_eq!(inbound_body["roomId"], "!ops:matrix.test");
    assert_eq!(inbound_body["senderMxid"], "@alex:matrix.test");
    assert_eq!(inbound_body["replyTo"], "msg-parent");
}

#[test]
fn agentd_http_backend_polls_outbox_and_maps_payload() {
    let server = FakeAgentdServer::new(vec![FakeResponse::status(
        200,
        json!({
            "events": [{
                "seq": 8,
                "event": "message",
                "created_at": 123,
                "payload": {
                    "messageId": "msg-8",
                    "source": "api",
                    "target": "codex-worker",
                    "roomId": "!ops:matrix.test",
                    "full": "full body",
                    "summary": "summary body"
                }
            }]
        })
        .to_string(),
    )]);
    let config = BridgeConfig::new(server.base_url()).expect("config");
    let mut backend = AgentdHttpBackend::new(&config).expect("http backend");

    let events = backend.poll_outbox(7).expect("outbox");

    let requests = server.requests();
    server.join();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/matrix/outbox?from_seq=7");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].seq, 8);
    assert_eq!(events[0].room_id.as_deref(), Some("!ops:matrix.test"));
    assert_eq!(events[0].target.as_deref(), Some("codex-worker"));
    assert_eq!(events[0].message_id.as_deref(), Some("msg-8"));
    assert_eq!(events[0].source.as_deref(), Some("api"));
    assert_eq!(events[0].body, "full body");
    assert_eq!(events[0].payload["summary"], "summary body");
}

#[test]
fn agentd_http_backend_reads_bot_command_snapshot_from_agents_and_groups() {
    let server = FakeAgentdServer::new(vec![
        FakeResponse::status(
            200,
            json!([
                {
                    "name": "codex-worker",
                    "status": "online",
                    "role": "coding",
                    "capability": "strong",
                    "runtime": "codex"
                },
                {
                    "name": "codex-reviewer",
                    "status": "offline",
                    "role": "review",
                    "capability": "medium",
                    "runtime": "codex"
                }
            ])
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!([
                {
                    "name": "ops",
                    "members": ["codex-worker", "codex-reviewer"]
                }
            ])
            .to_string(),
        ),
    ]);
    let config = BridgeConfig::new(server.base_url())
        .expect("config")
        .with_operator_token("bridge-secret");
    let backend = AgentdHttpBackend::new(&config).expect("http backend");

    let snapshot = backend
        .bot_command_snapshot()
        .expect("bot command snapshot");

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/agents");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, "/api/groups");
    assert_eq!(
        requests[1].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(snapshot.agents.len(), 2);
    assert_eq!(snapshot.agents[0].name, "codex-worker");
    assert_eq!(snapshot.agents[0].status, "online");
    assert_eq!(snapshot.agents[0].role.as_deref(), Some("coding"));
    assert_eq!(snapshot.agents[0].capability.as_deref(), Some("strong"));
    assert_eq!(snapshot.agents[0].runtime.as_deref(), Some("codex"));
    assert_eq!(snapshot.groups.len(), 1);
    assert_eq!(snapshot.groups[0].name, "ops");
    assert_eq!(
        snapshot.groups[0].members,
        ["codex-worker", "codex-reviewer"]
    );
}

#[test]
fn agentd_http_backend_executes_bot_management_effect_requests() {
    let server = FakeAgentdServer::new(vec![
        FakeResponse::status(
            200,
            json!({
                "name": "codex-worker",
                "status": "online",
                "role": "coding",
                "capability": "strong",
                "runtime": "codex"
            })
            .to_string(),
        ),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        FakeResponse::status(404, json!({"error": "not found"}).to_string()),
    ]);
    let config = BridgeConfig::new(server.base_url())
        .expect("config")
        .with_operator_token("bridge-secret");
    let mut backend = AgentdHttpBackend::new(&config).expect("http backend");

    let found = backend
        .lookup_bot_agent("codex-worker")
        .expect("lookup existing agent")
        .expect("agent found");
    let update = backend
        .update_bot_agent_identity("codex-worker", "Be concise")
        .expect("identity update");
    let missing = backend.lookup_bot_agent("ghost").expect("lookup missing");

    let requests = server.requests();
    server.join();
    assert_eq!(found.name, "codex-worker");
    assert_eq!(found.status, "online");
    assert!(update.error.is_none());
    assert!(missing.is_none());
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/agents/codex-worker");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(requests[1].method, "PATCH");
    assert_eq!(requests[1].path, "/api/agents/codex-worker");
    assert_eq!(
        requests[1].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(requests[1].header("content-type"), Some("application/json"));
    let patch_body: serde_json::Value =
        serde_json::from_str(&requests[1].body).expect("patch body");
    assert_eq!(patch_body["identity"], "Be concise");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].path, "/api/agents/ghost");
}

#[test]
fn agentd_http_backend_executes_group_management_effect_requests() {
    let server = FakeAgentdServer::new(vec![
        FakeResponse::status(
            201,
            json!({
                "ok": true,
                "group": {
                    "name": "ops",
                    "members": ["codex-worker", "codex-reviewer"],
                    "created_at": 1
                }
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "ok": true,
                "group": {
                    "name": "ops",
                    "members": ["codex-worker", "codex-reviewer"],
                    "created_at": 1
                }
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "name": "ops",
                "members": ["codex-worker", "codex-reviewer"],
                "created_at": 1
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "ok": true,
                "group": {
                    "name": "ops",
                    "members": ["codex-worker", "codex-reviewer"],
                    "created_at": 1
                }
            })
            .to_string(),
        ),
    ]);
    let config = BridgeConfig::new(server.base_url())
        .expect("config")
        .with_operator_token("bridge-secret");
    let mut backend = AgentdHttpBackend::new(&config).expect("http backend");

    let create = backend
        .create_bot_group(
            "ops",
            &["codex-worker".to_owned(), "codex-reviewer".to_owned()],
        )
        .expect("create group");
    let update = backend
        .update_bot_group_members("ops", &["codex-reviewer".to_owned()], &[])
        .expect("update group members");
    let found = backend
        .lookup_bot_group("ops")
        .expect("lookup group")
        .expect("group found");
    let delete = backend.delete_bot_group("ops").expect("delete group");

    let requests = server.requests();
    server.join();
    assert!(create.error.is_none());
    assert!(update.error.is_none());
    assert_eq!(found.name, "ops");
    assert_eq!(found.members, ["codex-worker", "codex-reviewer"]);
    assert!(delete.error.is_none());
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/groups");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/api/groups/ops/members");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].path, "/api/groups/ops");
    assert_eq!(requests[3].method, "DELETE");
    assert_eq!(requests[3].path, "/api/groups/ops");
    for request in &requests {
        assert_eq!(
            request.header("authorization"),
            Some("Bearer bridge-secret")
        );
    }
    let create_body: serde_json::Value =
        serde_json::from_str(&requests[0].body).expect("create body");
    assert_eq!(create_body["name"], "ops");
    assert_eq!(
        create_body["members"],
        json!(["codex-worker", "codex-reviewer"])
    );
    let update_body: serde_json::Value =
        serde_json::from_str(&requests[1].body).expect("member update body");
    assert_eq!(update_body["add"], json!(["codex-reviewer"]));
    assert_eq!(update_body["remove"], json!([]));
}

#[test]
fn agentd_http_backend_executes_joingroup_member_update_request() {
    let server = FakeAgentdServer::new(vec![FakeResponse::status(
        200,
        json!({
            "ok": true,
            "group": {
                "name": "ops",
                "members": ["codex-worker", "alex"],
                "created_at": 1
            }
        })
        .to_string(),
    )]);
    let config = BridgeConfig::new(server.base_url())
        .expect("config")
        .with_operator_token("bridge-secret");
    let mut backend = AgentdHttpBackend::new(&config).expect("http backend");

    let update = backend
        .update_bot_group_members("ops", &["alex".to_owned()], &[])
        .expect("joingroup member update");

    let requests = server.requests();
    server.join();
    assert!(update.error.is_none());
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/groups/ops/members");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    let body: serde_json::Value = serde_json::from_str(&requests[0].body).expect("body");
    assert_eq!(body["add"], json!(["alex"]));
    assert_eq!(body["remove"], json!([]));
}

#[test]
fn agentd_http_backend_reports_non_success_status_and_invalid_json() {
    let failing_server =
        FakeAgentdServer::new(vec![FakeResponse::status(500, r#"{"error":"boom"}"#)]);
    let failing_config = BridgeConfig::new(failing_server.base_url()).expect("config");
    let mut failing_backend = AgentdHttpBackend::new(&failing_config).expect("http backend");

    let status_err = failing_backend
        .register_room(group_room())
        .expect_err("status error");
    let status_text = status_err.to_string();
    failing_server.join();
    assert!(
        status_text.contains("status 500"),
        "error should mention status: {status_text}"
    );

    let invalid_server = FakeAgentdServer::new(vec![FakeResponse::status(200, "not json")]);
    let invalid_config = BridgeConfig::new(invalid_server.base_url()).expect("config");
    let mut invalid_backend = AgentdHttpBackend::new(&invalid_config).expect("http backend");

    let json_err = invalid_backend.poll_outbox(0).expect_err("json error");
    let json_text = json_err.to_string();
    invalid_server.join();
    assert!(
        json_text.contains("decode JSON"),
        "error should mention decode JSON: {json_text}"
    );
}

#[test]
fn bridge_state_json_persists_cursor_and_defaults_missing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("matrix").join("state.json");

    let missing = BridgeState::load_json(&path).expect("missing state defaults");
    assert_eq!(missing.next_from_seq(), 0);

    BridgeState::new(42).save_json(&path).expect("save state");

    let written = std::fs::read_to_string(&path).expect("read state");
    assert!(written.contains("nextFromSeq"), "{written}");
    assert!(written.contains("42"), "{written}");
    let loaded = BridgeState::load_json(&path).expect("load saved state");
    assert_eq!(loaded.next_from_seq(), 42);
}

#[derive(Debug, Default)]
struct FakeTransport {
    sent: Vec<MatrixOutboundEvent>,
}

impl MatrixBridgeTransport for FakeTransport {
    fn room_registrations(
        &mut self,
    ) -> Result<Vec<MatrixRoomRegistration>, agentd_matrix::BridgeError> {
        Ok(Vec::new())
    }

    fn inbound_events(&mut self) -> Result<Vec<MatrixInboundEvent>, agentd_matrix::BridgeError> {
        Ok(Vec::new())
    }

    fn send_outbound(
        &mut self,
        event: MatrixOutboundEvent,
    ) -> Result<(), agentd_matrix::BridgeError> {
        self.sent.push(event);
        Ok(())
    }
}

#[test]
fn matrix_runtime_with_http_backend_sends_outbox_and_persists_cursor() {
    let server = FakeAgentdServer::new(vec![FakeResponse::status(
        200,
        json!({
            "events": [
                {
                    "seq": 1,
                    "event": "message",
                    "created_at": 123,
                    "payload": {
                        "messageId": "msg-1",
                        "source": "api",
                        "target": "codex-worker",
                        "roomId": "!ops:matrix.test",
                        "full": "first"
                    }
                },
                {
                    "seq": 2,
                    "event": "message",
                    "created_at": 124,
                    "payload": {
                        "messageId": "msg-2",
                        "source": "api",
                        "target": "codex-worker",
                        "roomId": "!ops:matrix.test",
                        "summary": "second"
                    }
                }
            ]
        })
        .to_string(),
    )]);
    let config = BridgeConfig::new(server.base_url()).expect("config");
    let backend = AgentdHttpBackend::new(&config).expect("http backend");
    let transport = FakeTransport::default();
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let state = BridgeState::load_json(&state_path).expect("load state");
    let mut runtime = BridgeRuntime::new(backend, transport, state);

    let report = runtime.run_once().expect("run once");
    runtime
        .state()
        .save_json(&state_path)
        .expect("save confirmed cursor");

    server.join();
    assert_eq!(report.outbound_sent, 2);
    assert_eq!(runtime.transport().sent.len(), 2);
    assert_eq!(runtime.transport().sent[0].body, "first");
    assert_eq!(runtime.transport().sent[1].body, "second");
    assert_eq!(
        BridgeState::load_json(&state_path)
            .expect("reload")
            .next_from_seq(),
        2
    );
}
