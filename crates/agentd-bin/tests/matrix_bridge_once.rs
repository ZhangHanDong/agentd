use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_bin::matrix_bridge::run_matrix_bridge_once;
use agentd_bin::{DaemonConfig, MatrixBridgeOnceArgs};
use agentd_matrix::{BridgeError, MatrixPuppetAccountOutcome};
use serde_json::{Value, json};

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

fn write_json(path: &Path, value: &Value) {
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).expect("encode json"),
    )
    .expect("write json");
}

fn daemon_config(api_token: Option<&str>) -> DaemonConfig {
    DaemonConfig {
        security_mode: agentd_bin::SecurityRuntimeMode::Standalone,
        db_path: PathBuf::from("agentd.db"),
        port: 8787,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: PathBuf::from("."),
        worktree_base: PathBuf::from(".agentd/worktrees"),
        log_level: "info".to_owned(),
        api_token: api_token.map(ToOwned::to_owned),
        agent_tokens: Vec::new(),
        agent_token_mode: "audit".to_owned(),
        enterprise: Default::default(),
    }
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

fn bridge_runtime_responses(seq: i64, summary: &str) -> Vec<FakeResponse> {
    vec![
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
                        "summary": summary
                    }
                }]
            })
            .to_string(),
        ),
    ]
}

fn write_direct_room_and_inbound(rooms_path: &Path, inbound_path: &Path) {
    write_json(
        rooms_path,
        &json!([{
            "room_id": "!codex-worker:matrix.test",
            "group_name": null,
            "agent_name": "codex-worker",
            "trusted": true,
            "trust_reason": "managed",
            "inviter_mxid": "@alex:matrix.test",
            "members": ["codex-worker"]
        }]),
    );
    write_json(
        inbound_path,
        &json!([{
            "event_id": "$event-1",
            "room_id": "!codex-worker:matrix.test",
            "sender_mxid": "@alex:matrix.test",
            "body": "please continue",
            "mentions": ["codex-worker"],
            "reply_to": null
        }]),
    );
}

fn write_agent_chat_puppet_state(path: &Path) {
    write_json(
        path,
        &json!({
            "botToken": "bot-token",
            "agentTokens": {"old-agent": "stale-token"},
            "roomGroupMap": {"!ops:matrix.test": "ops"}
        }),
    );
}

#[test]
fn agentd_bin_matrix_bridge_once_runs_against_fake_agentd() {
    let server = FakeAgentdServer::new(bridge_runtime_responses(12, "reply from codex"));
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let state_path = dir.path().join("state.json");
    let sent_path = dir.path().join("sent.jsonl");
    write_direct_room_and_inbound(&rooms_path, &inbound_path);
    let args = MatrixBridgeOnceArgs {
        agentd_api: server.base_url().to_owned(),
        state: state_path.clone(),
        rooms_json: rooms_path,
        inbound_json: inbound_path,
        sent_log_jsonl: sent_path.clone(),
        matrix_homeserver_url: None,
        matrix_server_name: None,
        matrix_agent_prefix: "ac_".to_owned(),
        matrix_agents: Vec::new(),
        matrix_skip_agents: Vec::new(),
        matrix_puppet_state: None,
        matrix_agent_password_secret: None,
        matrix_agent_password_template: None,
        matrix_allow_legacy_agent_password: false,
        matrix_registration_token: None,
    };

    let report = run_matrix_bridge_once(&daemon_config(Some("bridge-secret")), &args)
        .expect("run matrix bridge once");

    let requests = server.requests();
    server.join();
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 1);
    assert_eq!(report.run.outbound_sent, 1);
    assert_eq!(report.next_from_seq, 12);
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/api/matrix/rooms");
    assert_eq!(requests[1].method, "POST");
    assert_eq!(requests[1].path, "/api/matrix/inbound");
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].path, "/api/matrix/outbox?from_seq=0");
    let inbound_body: Value = serde_json::from_str(&requests[1].body).expect("inbound body");
    assert_eq!(inbound_body["body"], "please continue");

    let sent = std::fs::read_to_string(&sent_path).expect("sent log");
    let sent_event: Value = serde_json::from_str(sent.trim()).expect("sent event");
    assert_eq!(sent_event["seq"], 12);
    assert_eq!(sent_event["roomId"], "!codex-worker:matrix.test");
    assert_eq!(sent_event["body"], "reply from codex");
    let state = std::fs::read_to_string(&state_path).expect("state file");
    assert!(state.contains("\"nextFromSeq\": 12"), "{state}");
}

#[test]
fn agentd_bin_matrix_bridge_once_provisions_puppets_with_file_store() {
    let matrix_server = FakeAgentdServer::new(vec![matrix_puppet_login_response()]);
    let agentd_server =
        FakeAgentdServer::new(bridge_runtime_responses(14, "reply after provisioning"));
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let state_path = dir.path().join("state.json");
    let sent_path = dir.path().join("sent.jsonl");
    let puppet_state_path = dir.path().join("bridge-state.json");
    write_direct_room_and_inbound(&rooms_path, &inbound_path);
    write_agent_chat_puppet_state(&puppet_state_path);
    let args = MatrixBridgeOnceArgs {
        agentd_api: agentd_server.base_url().to_owned(),
        state: state_path.clone(),
        rooms_json: rooms_path,
        inbound_json: inbound_path,
        sent_log_jsonl: sent_path.clone(),
        matrix_homeserver_url: Some(matrix_server.base_url().to_owned()),
        matrix_server_name: Some("matrix.test".to_owned()),
        matrix_agent_prefix: "ac_".to_owned(),
        matrix_agents: vec!["codex-worker".to_owned()],
        matrix_skip_agents: Vec::new(),
        matrix_puppet_state: Some(puppet_state_path.clone()),
        matrix_agent_password_secret: Some("matrix-secret".to_owned()),
        matrix_agent_password_template: None,
        matrix_allow_legacy_agent_password: false,
        matrix_registration_token: None,
    };

    let report = run_matrix_bridge_once(&daemon_config(Some("bridge-secret")), &args)
        .expect("run matrix bridge once");

    let matrix_requests = matrix_server.requests();
    let agentd_requests = agentd_server.requests();
    matrix_server.join();
    agentd_server.join();
    assert_eq!(matrix_requests.len(), 1);
    assert_eq!(matrix_requests[0].method, "POST");
    assert_eq!(matrix_requests[0].path, "/_matrix/client/v3/login");
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
    assert_eq!(agentd_requests.len(), 3);
    assert_eq!(
        agentd_requests[0].header("authorization"),
        Some("Bearer bridge-secret")
    );
    assert_eq!(agentd_requests[0].path, "/api/matrix/rooms");
    assert_eq!(agentd_requests[1].path, "/api/matrix/inbound");
    assert_eq!(agentd_requests[2].path, "/api/matrix/outbox?from_seq=0");
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 1);
    assert_eq!(report.run.outbound_sent, 1);
    assert_eq!(report.next_from_seq, 14);

    let puppet_state: Value = serde_json::from_str(
        &std::fs::read_to_string(&puppet_state_path).expect("read puppet state"),
    )
    .expect("decode puppet state");
    assert_eq!(puppet_state["agentTokens"]["codex-worker"], "codex-token");
    assert!(puppet_state["agentTokens"].get("old-agent").is_none());
    assert_eq!(puppet_state["botToken"], "bot-token");
    assert_eq!(puppet_state["roomGroupMap"]["!ops:matrix.test"], "ops");
    let sent = std::fs::read_to_string(&sent_path).expect("sent log");
    let sent_event: Value = serde_json::from_str(sent.trim()).expect("sent event");
    assert_eq!(sent_event["body"], "reply after provisioning");
}

#[test]
fn agentd_bin_matrix_bridge_once_rejects_incomplete_puppet_config_without_backend_contact() {
    let dir = tempfile::tempdir().expect("tempdir");
    let rooms_path = dir.path().join("rooms.json");
    let inbound_path = dir.path().join("inbound.json");
    let state_path = dir.path().join("state.json");
    let sent_path = dir.path().join("sent.jsonl");
    let puppet_state_path = dir.path().join("bridge-state.json");
    write_json(&rooms_path, &json!([]));
    write_json(&inbound_path, &json!([]));
    let args = MatrixBridgeOnceArgs {
        agentd_api: "http://127.0.0.1:1".to_owned(),
        state: state_path.clone(),
        rooms_json: rooms_path,
        inbound_json: inbound_path,
        sent_log_jsonl: sent_path.clone(),
        matrix_homeserver_url: Some("http://127.0.0.1:8008".to_owned()),
        matrix_server_name: None,
        matrix_agent_prefix: "ac_".to_owned(),
        matrix_agents: Vec::new(),
        matrix_skip_agents: Vec::new(),
        matrix_puppet_state: None,
        matrix_agent_password_secret: Some("matrix-secret".to_owned()),
        matrix_agent_password_template: None,
        matrix_allow_legacy_agent_password: false,
        matrix_registration_token: None,
    };

    let err = run_matrix_bridge_once(&daemon_config(None), &args)
        .expect_err("incomplete puppet config is rejected");

    assert!(matches!(err, BridgeError::InvalidConfig(_)));
    let message = err.to_string();
    assert!(
        message.contains("matrix-server-name"),
        "error names missing matrix server name: {message}"
    );
    assert!(!state_path.exists(), "cursor state must not be created");
    assert!(!sent_path.exists(), "sent log must not be created");
    assert!(
        !puppet_state_path.exists(),
        "puppet token state must not be created"
    );
}
