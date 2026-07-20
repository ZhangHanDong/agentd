use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_bin::matrix_bridge::{
    matrix_client_bridge_service_config, run_matrix_client_bridge_service,
    run_matrix_sdk_bridge_service,
};
use agentd_bin::{AgentdCli, AgentdCommand, DaemonConfig, MatrixClientBridgeServiceArgs};
use agentd_matrix::{
    BridgeError, MatrixClientPort, MatrixClientRoom, MatrixClientSync, MatrixClientTextMessage,
};
use clap::Parser;
use serde_json::{Value, json};

#[derive(Debug, Clone)]
struct CapturedRequest {
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

#[derive(Debug, Clone, Default)]
struct SharedFakeMatrixClient {
    state: Arc<Mutex<FakeMatrixState>>,
}

#[derive(Debug, Default)]
struct FakeMatrixState {
    calls: Vec<String>,
    syncs: VecDeque<MatrixClientSync>,
    sent: Vec<(String, String)>,
    fail_send_body: Option<String>,
}

impl SharedFakeMatrixClient {
    fn new(syncs: Vec<MatrixClientSync>) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeMatrixState {
                syncs: syncs.into(),
                ..FakeMatrixState::default()
            })),
        }
    }

    fn with_fail_send_body(self, body: impl Into<String>) -> Self {
        self.state.lock().expect("fake state").fail_send_body = Some(body.into());
        self
    }

    fn calls(&self) -> Vec<String> {
        self.state.lock().expect("fake state").calls.clone()
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.state.lock().expect("fake state").sent.clone()
    }
}

impl MatrixClientPort for SharedFakeMatrixClient {
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError> {
        self.state
            .lock()
            .expect("fake state")
            .calls
            .push("login".to_owned());
        Ok("@agentd-bot:matrix.test".to_owned())
    }

    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state.calls.push("sync".to_owned());
        state
            .syncs
            .pop_front()
            .ok_or_else(|| BridgeError::transport("unexpected extra sync"))
    }

    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.state
            .lock()
            .expect("fake state")
            .calls
            .push(format!("join:{room_id}"));
        Ok(())
    }

    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.state
            .lock()
            .expect("fake state")
            .calls
            .push(format!("leave:{room_id}"));
        Ok(())
    }

    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state.calls.push(format!("send:{room_id}:{body}"));
        state.sent.push((room_id.to_owned(), body.to_owned()));
        if state.fail_send_body.as_deref() == Some(body) {
            return Err(BridgeError::transport(format!("send failed for {body}")));
        }
        Ok(())
    }
}

fn daemon_config(api_token: Option<&str>) -> DaemonConfig {
    DaemonConfig {
        db_path: PathBuf::from("agentd.db"),
        port: 8787,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: PathBuf::from("."),
        worktree_base: PathBuf::from(".agentd/worktrees"),
        log_level: "info".to_owned(),
        api_token: api_token.map(ToOwned::to_owned),
        agent_tokens: Vec::new(),
        agent_token_mode: "audit".to_owned(),
        accept_workflow_change: false,
    }
}

fn service_args(
    server: &FakeAgentdServer,
    state_path: PathBuf,
    iterations: usize,
) -> MatrixClientBridgeServiceArgs {
    MatrixClientBridgeServiceArgs {
        agentd_api: server.base_url().to_owned(),
        state: state_path,
        iterations,
        matrix_homeserver_url: Some("http://127.0.0.1:8008".to_owned()),
        matrix_username: Some("agentd-bot".to_owned()),
        matrix_password: Some("bot-secret".to_owned()),
        matrix_user_id: None,
        matrix_device_id: None,
        matrix_access_token: None,
        matrix_sync_timeout_ms: 0,
        matrix_sdk_store: None,
        matrix_bot_user_id: Some("@agentd-bot:matrix.test".to_owned()),
        matrix_server_name: Some("matrix.test".to_owned()),
        matrix_agent_prefix: "ac_".to_owned(),
        matrix_agents: vec!["codex-worker".to_owned()],
        matrix_skip_agents: Vec::new(),
        matrix_trust_mode: "audit".to_owned(),
        matrix_trusted_inviters: vec!["@alex:matrix.test".to_owned()],
        matrix_ignored_senders: Vec::new(),
        matrix_operator_mxids: Vec::new(),
        matrix_admin_mxids: Vec::new(),
        matrix_puppet_state: None,
        matrix_agent_password_secret: None,
        matrix_agent_password_template: None,
        matrix_allow_legacy_agent_password: false,
        matrix_registration_token: None,
    }
}

#[test]
fn agentd_cli_matrix_client_bridge_service_accepts_bot_command_acl_options() {
    let bridge_service_command_line = [
        "agentd",
        "matrix-client-bridge-service",
        "--agentd-api",
        "http://127.0.0.1:8787",
        "--state",
        "state/matrix-client.json",
        "--matrix-homeserver-url",
        "http://127.0.0.1:8008",
        "--matrix-username",
        "agentd-bot",
        "--matrix-password",
        "bot-secret",
        "--matrix-operator",
        "@operator:matrix.test",
        "--matrix-operator",
        "@backup:matrix.test",
        "--matrix-admin",
        "@admin:matrix.test",
    ];
    let parsed_service =
        AgentdCli::try_parse_from(bridge_service_command_line).expect("service cli parses");
    let AgentdCommand::MatrixClientBridgeService(bridge_options) =
        parsed_service.command.as_ref().expect("service command")
    else {
        panic!("expected matrix-client-bridge-service command");
    };

    assert_eq!(
        bridge_options.matrix_operator_mxids,
        vec!["@operator:matrix.test", "@backup:matrix.test"]
    );
    assert_eq!(
        bridge_options.matrix_admin_mxids,
        vec!["@admin:matrix.test"]
    );
    let service_config = matrix_client_bridge_service_config(&daemon_config(None), bridge_options)
        .expect("service config");
    assert_eq!(
        service_config
            .once
            .transport_config
            .bot_command_acl
            .operator_mxids,
        vec!["@operator:matrix.test", "@backup:matrix.test"]
    );
    assert_eq!(
        service_config
            .once
            .transport_config
            .bot_command_acl
            .admin_mxids,
        vec!["@admin:matrix.test"]
    );

    let converted_command_line = bridge_service_command_line
        .iter()
        .map(|arg| match *arg {
            "matrix-client-bridge-service" => "matrix-client-bridge-preflight",
            other => other,
        })
        .collect::<Vec<_>>();
    let parsed_preflight =
        AgentdCli::try_parse_from(converted_command_line).expect("preflight cli parses");
    let AgentdCommand::MatrixClientBridgePreflight(check_options) = parsed_preflight
        .command
        .as_ref()
        .expect("preflight command")
    else {
        panic!("expected matrix-client-bridge-preflight command");
    };

    assert_eq!(
        check_options.service.matrix_operator_mxids,
        vec!["@operator:matrix.test", "@backup:matrix.test"]
    );
    assert_eq!(
        check_options.service.matrix_admin_mxids,
        vec!["@admin:matrix.test"]
    );
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

fn cursor_response() -> FakeResponse {
    FakeResponse::status(200, json!({"lastSeq": 0}).to_string())
}

fn ok_response() -> FakeResponse {
    FakeResponse::status(200, json!({"ok": true}).to_string())
}

/// Two gated iterations. `second_iteration_acks` controls whether the second
/// iteration reaches its outbox acknowledgement (false when the Matrix send is
/// scripted to fail before the ack request is issued).
fn bridge_runtime_responses(
    first_seq: i64,
    first_summary: &str,
    second_seq: i64,
    second_summary: &str,
    second_iteration_acks: bool,
) -> Vec<FakeResponse> {
    let mut responses = vec![
        native_caps_response(),
        cursor_response(),
        ok_response(),
        FakeResponse::status(201, json!({"ok": true}).to_string()),
        FakeResponse::status(200, outbox_response(first_seq, first_summary).to_string()),
        ok_response(),
        native_caps_response(),
        cursor_response(),
        ok_response(),
        FakeResponse::status(200, outbox_response(second_seq, second_summary).to_string()),
    ];
    if second_iteration_acks {
        responses.push(ok_response());
    }
    responses
}

fn outbox_response(seq: i64, summary: &str) -> Value {
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
}

fn sync_with_inbound(event_id: &str) -> MatrixClientSync {
    MatrixClientSync {
        joined_rooms: vec![direct_room()],
        text_events: vec![MatrixClientTextMessage {
            event_id: event_id.to_owned(),
            room_id: "!codex-worker:matrix.test".to_owned(),
            sender_mxid: "@alex:matrix.test".to_owned(),
            body: "please continue".to_owned(),
            formatted_body: None,
            mentions: vec!["codex-worker".to_owned()],
            reply_to: None,
        }],
        ..MatrixClientSync::default()
    }
}

fn sync_with_command(event_id: &str, body: &str) -> MatrixClientSync {
    MatrixClientSync {
        joined_rooms: vec![direct_room()],
        text_events: vec![MatrixClientTextMessage {
            event_id: event_id.to_owned(),
            room_id: "!codex-worker:matrix.test".to_owned(),
            sender_mxid: "@alex:matrix.test".to_owned(),
            body: body.to_owned(),
            formatted_body: None,
            mentions: Vec::new(),
            reply_to: None,
        }],
        ..MatrixClientSync::default()
    }
}

fn sync_without_inbound() -> MatrixClientSync {
    MatrixClientSync {
        joined_rooms: vec![direct_room()],
        ..MatrixClientSync::default()
    }
}

fn direct_room() -> MatrixClientRoom {
    MatrixClientRoom {
        room_id: "!codex-worker:matrix.test".to_owned(),
        group_name: None,
        agent_name: Some("codex-worker".to_owned()),
        trusted: true,
        trust_reason: "managed".to_owned(),
        inviter_mxid: Some("@alex:matrix.test".to_owned()),
        members: vec!["codex-worker".to_owned()],
    }
}

fn read_state(path: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(path).expect("state file")).expect("state json")
}

#[test]
fn agentd_bin_matrix_client_bridge_service_runs_bounded_iterations_with_fake_client() {
    let server = FakeAgentdServer::new(bridge_runtime_responses(
        21,
        "first reply",
        22,
        "second reply",
        true,
    ));
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let args = service_args(&server, state_path.clone(), 2);
    let service_config =
        matrix_client_bridge_service_config(&daemon_config(Some("bridge-secret")), &args)
            .expect("service config");
    let mut client =
        SharedFakeMatrixClient::new(vec![sync_with_inbound("$event-1"), sync_without_inbound()]);

    let report =
        run_matrix_client_bridge_service(&service_config, &mut client).expect("run service");

    assert_eq!(2, report.iterations.len());
    assert_eq!(22, report.next_from_seq);
    assert_eq!(
        client.calls(),
        vec![
            "login",
            "sync",
            "send:!codex-worker:matrix.test:first reply",
            "login",
            "sync",
            "send:!codex-worker:matrix.test:second reply",
        ]
    );
    assert_eq!(
        client.sent(),
        vec![
            (
                "!codex-worker:matrix.test".to_owned(),
                "first reply".to_owned()
            ),
            (
                "!codex-worker:matrix.test".to_owned(),
                "second reply".to_owned()
            ),
        ]
    );

    let requests = server.requests();
    server.join();
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/runtime/capabilities");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].path,
        "/api/matrix/outbox/cursor?bridgeId=matrix-bridge"
    );
    assert_eq!(requests[2].method, "POST");
    assert_eq!(requests[2].path, "/api/matrix/rooms");
    assert_eq!(requests[3].method, "POST");
    assert_eq!(requests[3].path, "/api/matrix/inbound");
    assert!(requests[3].body.contains("$event-1"));
    assert_eq!(requests[4].method, "GET");
    assert_eq!(requests[4].path, "/api/matrix/outbox?from_seq=0");
    assert_eq!(requests[5].method, "POST");
    assert_eq!(requests[5].path, "/api/matrix/outbox/ack");
    assert_eq!(requests[6].method, "GET");
    assert_eq!(requests[6].path, "/api/runtime/capabilities");
    assert_eq!(requests[7].method, "GET");
    assert_eq!(
        requests[7].path,
        "/api/matrix/outbox/cursor?bridgeId=matrix-bridge"
    );
    assert_eq!(requests[8].method, "POST");
    assert_eq!(requests[8].path, "/api/matrix/rooms");
    assert_eq!(requests[9].method, "GET");
    assert_eq!(requests[9].path, "/api/matrix/outbox?from_seq=21");
    assert_eq!(requests[10].method, "POST");
    assert_eq!(requests[10].path, "/api/matrix/outbox/ack");
    assert_eq!(read_state(&state_path)["nextFromSeq"], 22);
}

#[test]
fn agentd_bin_matrix_client_bridge_service_reports_bot_command_reply_counts() {
    let server = FakeAgentdServer::new(vec![
        native_caps_response(),
        cursor_response(),
        FakeResponse::status(
            200,
            json!([
                {
                    "name": "codex-worker",
                    "status": "online",
                    "role": "coding",
                    "capability": "strong",
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
                    "members": ["codex-worker"]
                }
            ])
            .to_string(),
        ),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        FakeResponse::status(200, json!({"events": []}).to_string()),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let args = service_args(&server, state_path.clone(), 1);
    let service_config =
        matrix_client_bridge_service_config(&daemon_config(Some("bridge-secret")), &args)
            .expect("service config");
    let mut client = SharedFakeMatrixClient::new(vec![sync_with_command("$cmd", "!status")]);

    let report =
        run_matrix_client_bridge_service(&service_config, &mut client).expect("run service");

    let requests = server.requests();
    server.join();
    assert_eq!(1, report.iterations.len());
    assert_eq!(0, report.next_from_seq);
    assert_eq!(1, report.bot_command_replies_sent);
    assert_eq!(1, report.iterations[0].run.bot_command_replies_sent);
    assert_eq!(0, report.iterations[0].run.inbound_forwarded);
    assert_eq!(0, report.iterations[0].run.outbound_sent);
    assert_eq!(client.sent().len(), 1);
    assert!(client.sent()[0].1.contains("=== System Status ==="));
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/api/runtime/capabilities");
    assert_eq!(requests[1].method, "GET");
    assert_eq!(
        requests[1].path,
        "/api/matrix/outbox/cursor?bridgeId=matrix-bridge"
    );
    assert_eq!(requests[2].method, "GET");
    assert_eq!(requests[2].path, "/api/agents");
    assert_eq!(requests[3].method, "GET");
    assert_eq!(requests[3].path, "/api/groups");
    assert_eq!(requests[4].method, "POST");
    assert_eq!(requests[4].path, "/api/matrix/rooms");
    assert_eq!(requests[5].method, "GET");
    assert_eq!(requests[5].path, "/api/matrix/outbox?from_seq=0");
    assert!(
        !requests
            .iter()
            .any(|request| request.path == "/api/matrix/inbound")
    );
    assert_eq!(read_state(&state_path)["nextFromSeq"], 0);
}

#[test]
fn agentd_bin_matrix_client_bridge_service_preserves_cursor_when_later_iteration_send_fails() {
    let server = FakeAgentdServer::new(bridge_runtime_responses(
        21,
        "first reply",
        22,
        "second reply",
        false,
    ));
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let args = service_args(&server, state_path.clone(), 2);
    let service_config =
        matrix_client_bridge_service_config(&daemon_config(None), &args).expect("service config");
    let mut client =
        SharedFakeMatrixClient::new(vec![sync_with_inbound("$event-1"), sync_without_inbound()])
            .with_fail_send_body("second reply");

    let err = run_matrix_client_bridge_service(&service_config, &mut client)
        .expect_err("second iteration fails");

    server.join();
    assert!(matches!(err, BridgeError::Transport(_)));
    assert_eq!(
        client.sent(),
        vec![
            (
                "!codex-worker:matrix.test".to_owned(),
                "first reply".to_owned()
            ),
            (
                "!codex-worker:matrix.test".to_owned(),
                "second reply".to_owned()
            ),
        ]
    );
    assert_eq!(read_state(&state_path)["nextFromSeq"], 21);
}

#[test]
#[cfg(not(feature = "matrix-sdk-adapter"))]
fn agentd_bin_matrix_client_bridge_service_default_build_requires_sdk_feature() {
    let server = FakeAgentdServer::new(Vec::new());
    let dir = tempfile::tempdir().expect("tempdir");
    let args = service_args(&server, dir.path().join("state.json"), 1);

    let err = run_matrix_sdk_bridge_service(&daemon_config(None), &args)
        .expect_err("default build is feature-gated");

    assert!(matches!(err, BridgeError::InvalidConfig(_)));
    assert!(
        err.to_string().contains("matrix-sdk-adapter"),
        "error should name the required feature: {err}"
    );
    assert!(server.requests().is_empty());
    server.join();
}
