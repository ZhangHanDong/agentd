use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    BridgeConfig, BridgeOnceConfig, BridgeOncePuppetAccountConfig, FileMatrixTransport,
    MatrixBridgeTransport, MatrixInboundEvent, MatrixOutboundEvent, MatrixPuppetAccountOutcome,
    MatrixPuppetDirectory, MatrixPuppetHttpAccountConfig, MatrixPuppetProvisioningConfig,
    MatrixRoomDirectory, MatrixRoomRegistration, run_bridge_once,
};
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct CapturedRequest {
    order: usize,
    method: String,
    path: String,
    body: String,
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
        Self::new_with_sequence(responses, Arc::new(AtomicUsize::new(1)))
    }

    fn new_with_sequence(responses: Vec<FakeResponse>, sequence: Arc<AtomicUsize>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake server");
        let base_url = format!("http://{}", listener.local_addr().expect("local addr"));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let thread_requests = Arc::clone(&requests);

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept request");
                let mut request = read_request(&mut stream);
                request.order = sequence.fetch_add(1, Ordering::SeqCst);
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
        order: 0,
        method: parts.next().expect("method").to_owned(),
        path: parts.next().expect("path").to_owned(),
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
        members: vec!["codex-worker".to_owned(), "codex-reviewer".to_owned()],
    }
}

fn direct_room() -> MatrixRoomRegistration {
    MatrixRoomRegistration {
        room_id: "!codex-worker:matrix.test".to_owned(),
        group_name: None,
        agent_name: Some("codex-worker".to_owned()),
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned()],
    }
}

fn untrusted_room() -> MatrixRoomRegistration {
    MatrixRoomRegistration {
        trusted: false,
        ..direct_room()
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

fn outbound_event(seq: i64, room_id: Option<&str>, target: Option<&str>) -> MatrixOutboundEvent {
    MatrixOutboundEvent {
        seq,
        room_id: room_id.map(ToOwned::to_owned),
        target: target.map(ToOwned::to_owned),
        body: format!("body {seq}"),
        message_id: Some(format!("msg-{seq}")),
        source: Some("api".to_owned()),
        payload: json!({
            "messageId": format!("msg-{seq}"),
            "source": "api",
            "target": target,
            "roomId": room_id,
            "full": format!("body {seq}")
        }),
    }
}

fn write_json(path: &Path, value: &Value) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).expect("encode json"),
    )
    .expect("write json");
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("read json")).expect("decode json")
}

fn matrix_puppet_login_response() -> FakeResponse {
    FakeResponse::status(
        200,
        json!({
            "user_id": "@ac_codex-worker:matrix.test",
            "access_token": "codex-token"
        })
        .to_string(),
    )
}

fn native_caps_response() -> FakeResponse {
    FakeResponse::status(
        200,
        json!({
            "runtime": "native",
            "runtimeApiVersion": 1,
            "sessionResume": true,
            "artifactAcknowledgement": true
        })
        .to_string(),
    )
}

fn ack_response() -> FakeResponse {
    FakeResponse::status(200, json!({"ok": true}).to_string())
}

fn bridge_runtime_responses(seq: i64, body: &str) -> Vec<FakeResponse> {
    vec![
        native_caps_response(),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        FakeResponse::status(201, json!({"ok": true}).to_string()),
        FakeResponse::status(
            200,
            json!({
                "events": [{
                    "seq": seq,
                    "event": "message",
                    "created_at": 123,
                    "payload": {
                        "messageId": format!("msg-{seq}"),
                        "source": "api",
                        "target": "codex-worker",
                        "full": body
                    }
                }]
            })
            .to_string(),
        ),
        ack_response(),
    ]
}

fn bridge_once_puppet_account_config(
    matrix_homeserver_url: &str,
    token_state_path: &Path,
) -> BridgeOncePuppetAccountConfig {
    BridgeOncePuppetAccountConfig {
        directory: MatrixPuppetDirectory::new(
            "matrix.test",
            "ac_",
            ["codex-worker"],
            Vec::<&str>::new(),
        )
        .expect("puppet directory"),
        provisioning_config: MatrixPuppetProvisioningConfig {
            password_secret: Some("matrix-secret".to_owned()),
            ..MatrixPuppetProvisioningConfig::default()
        },
        http_account_config: MatrixPuppetHttpAccountConfig::new(matrix_homeserver_url)
            .expect("matrix http account config"),
        token_state_path: token_state_path.to_path_buf(),
    }
}

#[test]
fn matrix_room_directory_resolves_direct_room_and_trusted_target() {
    let directory = MatrixRoomDirectory::new(vec![group_room(), direct_room(), untrusted_room()]);

    let direct = directory
        .resolve_room_id(&outbound_event(
            1,
            Some("!explicit:matrix.test"),
            Some("missing"),
        ))
        .expect("explicit room id resolves");
    let by_group = directory
        .resolve_room_id(&outbound_event(2, None, Some("ops")))
        .expect("group target resolves");
    let by_agent = directory
        .resolve_room_id(&outbound_event(3, None, Some("codex-worker")))
        .expect("agent target resolves");

    assert_eq!(direct, "!explicit:matrix.test");
    assert_eq!(by_group, "!ops:matrix.test");
    assert_eq!(by_agent, "!codex-worker:matrix.test");
}

#[test]
fn file_matrix_transport_writes_sent_jsonl_with_resolved_room() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let sent_path = dir.path().join("out").join("sent.jsonl");
    write_json(&rooms_path, &json!([direct_room()]));
    write_json(&inbound_path, &json!([inbound_event()]));
    let mut transport =
        FileMatrixTransport::from_files(&rooms_path, &inbound_path, &sent_path).expect("transport");

    assert_eq!(transport.room_registrations().expect("rooms").len(), 1);
    assert_eq!(transport.inbound_events().expect("inbound").len(), 1);
    transport
        .send_outbound(outbound_event(9, None, Some("codex-worker")))
        .expect("send outbound");

    let written = std::fs::read_to_string(&sent_path).expect("read sent log");
    let lines: Vec<&str> = written.lines().collect();
    assert_eq!(lines.len(), 1);
    let event: Value = serde_json::from_str(lines[0]).expect("sent event json");
    assert_eq!(event["seq"], 9);
    assert_eq!(event["roomId"], "!codex-worker:matrix.test");
    assert_eq!(event["target"], "codex-worker");
    assert_eq!(event["messageId"], "msg-9");
    assert_eq!(event["source"], "api");
    assert_eq!(event["body"], "body 9");
    assert_eq!(event["payload"]["full"], "body 9");
}

#[test]
fn file_matrix_transport_rejects_unmapped_target_without_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let sent_path = dir.path().join("sent.jsonl");
    write_json(&rooms_path, &json!([group_room()]));
    write_json(&inbound_path, &json!([]));
    let mut transport =
        FileMatrixTransport::from_files(&rooms_path, &inbound_path, &sent_path).expect("transport");

    let err = transport
        .send_outbound(outbound_event(10, None, Some("unknown-agent")))
        .expect_err("unmapped target fails");

    let err_text = err.to_string();
    assert!(
        err_text.contains("unknown-agent"),
        "error should name target: {err_text}"
    );
    assert!(!sent_path.exists(), "failed send must not create sent log");
}

#[test]
fn matrix_bridge_once_runner_posts_files_polls_outbox_logs_sent_and_saves_cursor() {
    let server = FakeAgentdServer::new(vec![
        native_caps_response(),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        FakeResponse::status(201, json!({"ok": true}).to_string()),
        FakeResponse::status(
            200,
            json!({
                "events": [{
                    "seq": 11,
                    "event": "message",
                    "created_at": 123,
                    "payload": {
                        "messageId": "msg-11",
                        "source": "api",
                        "target": "codex-worker",
                        "full": "hello from agentd"
                    }
                }]
            })
            .to_string(),
        ),
        ack_response(),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let state_path = dir.path().join("state").join("matrix.json");
    let sent_path = dir.path().join("sent").join("matrix.jsonl");
    write_json(&rooms_path, &json!([direct_room()]));
    write_json(&inbound_path, &json!([inbound_event()]));
    let once_config = BridgeOnceConfig {
        bridge_config: BridgeConfig::new(server.base_url()).expect("bridge config"),
        state_path: state_path.clone(),
        rooms_json_path: rooms_path,
        inbound_json_path: inbound_path,
        sent_log_jsonl_path: sent_path.clone(),
        puppet_accounts: None,
    };

    let report = run_bridge_once(&once_config).expect("run bridge once");

    let requests = server.requests();
    server.join();
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 1);
    assert_eq!(report.run.outbound_sent, 1);
    assert_eq!(report.next_from_seq, 11);
    assert_eq!(requests.len(), 5);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/runtime/capabilities");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/api/matrix/rooms");
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].path, "/api/matrix/inbound");
    assert_eq!(requests[3].method, "GET");
    assert_eq!(requests[3].path, "/api/matrix/outbox?from_seq=0");
    assert_eq!(requests[4].method, "POST");
    assert_eq!(requests[4].path, "/api/matrix/outbox/ack");
    let room_body: Value = serde_json::from_str(&requests[1].body).expect("room body");
    assert_eq!(room_body["agent"], "codex-worker");
    let inbound_body: Value = serde_json::from_str(&requests[2].body).expect("inbound body");
    assert_eq!(inbound_body["eventId"], "$event-1");

    let sent = std::fs::read_to_string(&sent_path).expect("sent log");
    let sent_event: Value = serde_json::from_str(sent.trim()).expect("sent event");
    assert_eq!(sent_event["seq"], 11);
    assert_eq!(sent_event["roomId"], "!codex-worker:matrix.test");
    assert_eq!(sent_event["body"], "hello from agentd");
    let state = std::fs::read_to_string(&state_path).expect("state file");
    assert!(state.contains("\"nextFromSeq\": 11"), "{state}");
}

#[test]
fn matrix_bridge_once_runner_provisions_puppet_accounts_before_bridge_runtime() {
    let request_sequence = Arc::new(AtomicUsize::new(1));
    let matrix_server = FakeAgentdServer::new_with_sequence(
        vec![matrix_puppet_login_response()],
        Arc::clone(&request_sequence),
    );
    let agentd_server = FakeAgentdServer::new_with_sequence(
        bridge_runtime_responses(13, "hello after provisioning"),
        Arc::clone(&request_sequence),
    );
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let state_path = dir.path().join("state").join("matrix.json");
    let sent_path = dir.path().join("sent").join("matrix.jsonl");
    let puppet_state_path = dir.path().join("bridge-state.json");
    write_json(&rooms_path, &json!([direct_room()]));
    write_json(&inbound_path, &json!([inbound_event()]));
    write_json(
        &puppet_state_path,
        &json!({
            "botToken": "bot-token",
            "agentTokens": {"old-agent": "stale-token"},
            "groupRoomMap": {"ops": "!ops:matrix.test"}
        }),
    );
    let once_config = BridgeOnceConfig {
        bridge_config: BridgeConfig::new(agentd_server.base_url()).expect("bridge config"),
        state_path: state_path.clone(),
        rooms_json_path: rooms_path,
        inbound_json_path: inbound_path,
        sent_log_jsonl_path: sent_path.clone(),
        puppet_accounts: Some(bridge_once_puppet_account_config(
            matrix_server.base_url(),
            &puppet_state_path,
        )),
    };

    let report = run_bridge_once(&once_config).expect("run bridge once");

    let matrix_requests = matrix_server.requests();
    let agentd_requests = agentd_server.requests();
    matrix_server.join();
    agentd_server.join();
    assert_eq!(matrix_requests.len(), 1);
    assert_eq!(matrix_requests[0].method, "POST");
    assert_eq!(matrix_requests[0].path, "/_matrix/client/v3/login");
    assert!(
        matrix_requests[0].order < agentd_requests[0].order,
        "puppet provisioning should run before bridge backend calls"
    );
    let login_body: Value =
        serde_json::from_str(&matrix_requests[0].body).expect("login request body");
    assert_eq!(login_body["identifier"]["user"], "ac_codex-worker");
    let puppet_report = report
        .puppet_account_provisioning
        .as_ref()
        .expect("puppet account report");
    assert_eq!(
        puppet_report.outcomes(),
        &[MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(
        puppet_report.pruned_token_names(),
        &["old-agent".to_owned()]
    );
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 1);
    assert_eq!(report.run.outbound_sent, 1);
    assert_eq!(report.next_from_seq, 13);
    let puppet_state = read_json(&puppet_state_path);
    assert_eq!(puppet_state["agentTokens"]["codex-worker"], "codex-token");
    assert!(puppet_state["agentTokens"].get("old-agent").is_none());
    assert_eq!(puppet_state["botToken"], "bot-token");
    let sent = std::fs::read_to_string(&sent_path).expect("sent log");
    let sent_event: Value = serde_json::from_str(sent.trim()).expect("sent event");
    assert_eq!(sent_event["body"], "hello after provisioning");
    let state = std::fs::read_to_string(&state_path).expect("state file");
    assert!(state.contains("\"nextFromSeq\": 13"), "{state}");
}
