use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    BridgeConfig, BridgeError, BridgeOncePuppetAccountConfig, MatrixBotCommandAcl,
    MatrixBotDmRoomResult, MatrixBotDmRoomStatus, MatrixClientBridgeOnceConfig, MatrixClientPort,
    MatrixClientRoom, MatrixClientSync, MatrixClientTextMessage, MatrixClientTransportConfig,
    MatrixPuppetAccountOutcome, MatrixPuppetDirectory, MatrixPuppetHttpAccountConfig,
    MatrixPuppetProvisioningConfig, MatrixTrustMode, run_matrix_client_bridge_once,
};
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
struct FakeHttpServer {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: thread::JoinHandle<()>,
}

impl FakeHttpServer {
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
        let content_length = headers
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

#[derive(Debug, Clone, Default)]
struct FakeMatrixState {
    calls: Vec<String>,
    sync: MatrixClientSync,
    sent: Vec<(String, String)>,
    dm_requests: Vec<(String, String)>,
    dm_result: MatrixBotDmRoomResult,
    created_direct_rooms: Vec<(String, Vec<String>)>,
    next_direct_room_id: Option<String>,
    invited_users: Vec<(String, String)>,
    fail_send_body: Option<String>,
    required_agent_token: Option<(PathBuf, String, String)>,
}

impl SharedFakeMatrixClient {
    fn new(sync: MatrixClientSync) -> Self {
        Self {
            state: Arc::new(Mutex::new(FakeMatrixState {
                sync,
                ..FakeMatrixState::default()
            })),
        }
    }

    fn with_fail_send_body(self, body: impl Into<String>) -> Self {
        self.state.lock().expect("fake state").fail_send_body = Some(body.into());
        self
    }

    fn with_dm_result(self, result: MatrixBotDmRoomResult) -> Self {
        self.state.lock().expect("fake state").dm_result = result;
        self
    }

    fn with_required_agent_token(
        self,
        path: impl Into<PathBuf>,
        agent_name: impl Into<String>,
        access_token: impl Into<String>,
    ) -> Self {
        self.state.lock().expect("fake state").required_agent_token =
            Some((path.into(), agent_name.into(), access_token.into()));
        self
    }

    fn calls(&self) -> Vec<String> {
        self.state.lock().expect("fake state").calls.clone()
    }

    fn sent(&self) -> Vec<(String, String)> {
        self.state.lock().expect("fake state").sent.clone()
    }

    fn created_direct_rooms(&self) -> Vec<(String, Vec<String>)> {
        self.state
            .lock()
            .expect("fake state")
            .created_direct_rooms
            .clone()
    }

    fn invited_users(&self) -> Vec<(String, String)> {
        self.state.lock().expect("fake state").invited_users.clone()
    }
}

impl MatrixClientPort for SharedFakeMatrixClient {
    fn ensure_logged_in(&mut self) -> Result<String, BridgeError> {
        let required = {
            let mut state = self.state.lock().expect("fake state");
            state.calls.push("ensure_logged_in".to_owned());
            state.required_agent_token.clone()
        };
        if let Some((path, agent_name, access_token)) = required {
            let value = read_json(&path);
            let stored = value
                .get("agentTokens")
                .and_then(|tokens| tokens.get(&agent_name))
                .and_then(Value::as_str);
            if stored != Some(access_token.as_str()) {
                return Err(BridgeError::transport(
                    "fake Matrix client required puppet token before login",
                ));
            }
        }
        Ok("@agent-bridge:matrix.test".to_owned())
    }

    fn sync_once(&mut self) -> Result<MatrixClientSync, BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state.calls.push("sync_once".to_owned());
        Ok(state.sync.clone())
    }

    fn join_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.state
            .lock()
            .expect("fake state")
            .calls
            .push(format!("join_room:{room_id}"));
        Ok(())
    }

    fn leave_room(&mut self, room_id: &str) -> Result<(), BridgeError> {
        self.state
            .lock()
            .expect("fake state")
            .calls
            .push(format!("leave_room:{room_id}"));
        Ok(())
    }

    fn send_text_message(&mut self, room_id: &str, body: &str) -> Result<(), BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state
            .calls
            .push(format!("send_text_message:{room_id}:{body}"));
        if state.fail_send_body.as_deref() == Some(body) {
            return Err(BridgeError::transport(format!(
                "fake Matrix send failed in {room_id}"
            )));
        }
        state.sent.push((room_id.to_owned(), body.to_owned()));
        Ok(())
    }

    fn create_direct_room(
        &mut self,
        name: &str,
        invite_mxids: &[String],
    ) -> Result<String, BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state.calls.push(format!("create_direct_room:{name}"));
        state
            .created_direct_rooms
            .push((name.to_owned(), invite_mxids.to_vec()));
        state
            .next_direct_room_id
            .clone()
            .ok_or_else(|| BridgeError::transport("fake direct room id missing"))
    }

    fn invite_user_to_room(&mut self, room_id: &str, user_mxid: &str) -> Result<(), BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state
            .calls
            .push(format!("invite_user_to_room:{room_id}:{user_mxid}"));
        state
            .invited_users
            .push((room_id.to_owned(), user_mxid.to_owned()));
        Ok(())
    }

    fn ensure_human_dm_room(
        &mut self,
        agent_name: &str,
        human_mxid: &str,
    ) -> Result<MatrixBotDmRoomResult, BridgeError> {
        let mut state = self.state.lock().expect("fake state");
        state
            .calls
            .push(format!("ensure_human_dm_room:{agent_name}:{human_mxid}"));
        state
            .dm_requests
            .push((agent_name.to_owned(), human_mxid.to_owned()));
        Ok(state.dm_result.clone())
    }
}

fn transport_config() -> MatrixClientTransportConfig {
    MatrixClientTransportConfig {
        bot_user_id: None,
        agent_user_prefix: "ac_".to_owned(),
        matrix_server_name: Some("matrix.test".to_owned()),
        known_agent_names: vec!["codex-worker".to_owned()],
        skip_agent_names: Vec::new(),
        trust_mode: MatrixTrustMode::Audit,
        trusted_inviter_mxids: vec!["@alex:matrix.test".to_owned()],
        ignored_sender_mxids: Vec::new(),
        bot_command_acl: MatrixBotCommandAcl::default(),
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

fn inbound_text() -> MatrixClientTextMessage {
    MatrixClientTextMessage {
        event_id: "$event-1".to_owned(),
        room_id: "!codex-worker:matrix.test".to_owned(),
        sender_mxid: "@alex:matrix.test".to_owned(),
        body: "please continue".to_owned(),
        formatted_body: None,
        mentions: vec!["codex-worker".to_owned()],
        reply_to: None,
    }
}

fn command_text(body: &str) -> MatrixClientTextMessage {
    MatrixClientTextMessage {
        event_id: "$cmd".to_owned(),
        room_id: "!codex-worker:matrix.test".to_owned(),
        sender_mxid: "@alex:matrix.test".to_owned(),
        body: body.to_owned(),
        formatted_body: None,
        mentions: Vec::new(),
        reply_to: None,
    }
}

fn sync_with_direct_room(text_events: Vec<MatrixClientTextMessage>) -> MatrixClientSync {
    MatrixClientSync {
        invites: Vec::new(),
        joined_rooms: vec![direct_room()],
        text_events,
    }
}

fn bridge_config(server: &FakeHttpServer, state_path: PathBuf) -> MatrixClientBridgeOnceConfig {
    MatrixClientBridgeOnceConfig {
        bridge_config: BridgeConfig::new(server.base_url()).expect("bridge config"),
        state_path,
        transport_config: transport_config(),
        puppet_accounts: None,
    }
}

fn bridge_runtime_responses(seq: i64, body: &str) -> Vec<FakeResponse> {
    vec![
        native_caps_response(),
        cursor_response(),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        FakeResponse::status(201, json!({"ok": true}).to_string()),
        outbox_response(vec![(seq, body)]),
        ack_response(),
    ]
}

fn outbox_response(events: Vec<(i64, &str)>) -> FakeResponse {
    let events: Vec<Value> = events
        .into_iter()
        .map(|(seq, body)| {
            json!({
                "seq": seq,
                "event": "message",
                "created_at": 123,
                "payload": {
                    "messageId": format!("msg-{seq}"),
                    "source": "api",
                    "target": "codex-worker",
                    "summary": body,
                }
            })
        })
        .collect();
    FakeResponse::status(200, json!({"events": events}).to_string())
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

fn ack_response() -> FakeResponse {
    FakeResponse::status(200, json!({"ok": true}).to_string())
}

fn puppet_accounts(server: &FakeHttpServer, path: &Path) -> BridgeOncePuppetAccountConfig {
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
        http_account_config: MatrixPuppetHttpAccountConfig::new(server.base_url())
            .expect("http account config"),
        token_state_path: path.to_path_buf(),
    }
}

fn matrix_login_response() -> FakeResponse {
    FakeResponse::status(
        200,
        json!({
            "user_id": "@ac_codex-worker:matrix.test",
            "access_token": "codex-token"
        })
        .to_string(),
    )
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

#[test]
fn matrix_client_bridge_once_runner_syncs_client_posts_backend_sends_outbox_saves_cursor() {
    let agentd = FakeHttpServer::new(bridge_runtime_responses(21, "reply from codex"));
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state").join("matrix.json");
    let config = bridge_config(&agentd, state_path.clone());
    let client = SharedFakeMatrixClient::new(sync_with_direct_room(vec![inbound_text()]));

    let report = run_matrix_client_bridge_once(&config, client.clone()).expect("run client bridge");

    let requests = agentd.requests();
    agentd.join();
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 1);
    assert_eq!(report.run.outbound_sent, 1);
    assert_eq!(report.next_from_seq, 21);
    assert_eq!(client.calls()[0..2], ["ensure_logged_in", "sync_once"]);
    assert_eq!(
        client.sent(),
        vec![(
            "!codex-worker:matrix.test".to_owned(),
            "reply from codex".to_owned()
        )]
    );
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
    assert_eq!(requests[4].method, "GET");
    assert_eq!(requests[4].path, "/api/matrix/outbox?from_seq=0");
    assert_eq!(requests[5].method, "POST");
    assert_eq!(requests[5].path, "/api/matrix/outbox/ack");
    let inbound_body: Value = serde_json::from_str(&requests[3].body).expect("inbound body");
    assert_eq!(inbound_body["body"], "please continue");
    let state = read_json(&state_path);
    assert_eq!(state["nextFromSeq"], 21);
}

#[test]
fn matrix_client_bridge_once_executes_bot_command_replies_and_reports_count() {
    let agentd = FakeHttpServer::new(vec![
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
        outbox_response(Vec::new()),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let config = bridge_config(&agentd, state_path.clone());
    let client = SharedFakeMatrixClient::new(sync_with_direct_room(vec![command_text("!help")]));

    let report = run_matrix_client_bridge_once(&config, client.clone()).expect("run client bridge");

    let requests = agentd.requests();
    agentd.join();
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 0);
    assert_eq!(report.run.outbound_sent, 0);
    assert_eq!(report.run.bot_command_replies_sent, 1);
    assert_eq!(report.next_from_seq, 0);
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
    assert_eq!(client.sent().len(), 1);
    assert_eq!(client.sent()[0].0, "!codex-worker:matrix.test");
    assert!(
        client.sent()[0]
            .1
            .contains("=== Agent Bridge Bot Commands ===")
    );
    let state = read_json(&state_path);
    assert_eq!(state["nextFromSeq"], 0);
}

#[test]
fn matrix_client_bridge_once_executes_management_command_effects_and_reports_count() {
    let agentd = FakeHttpServer::new(vec![
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
        outbox_response(Vec::new()),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let config = bridge_config(&agentd, state_path.clone());
    let client = SharedFakeMatrixClient::new(sync_with_direct_room(vec![command_text(
        "!dm codex-worker",
    )]))
    .with_dm_result(MatrixBotDmRoomResult {
        room_id: Some("!dm-codex-worker:matrix.test".to_owned()),
        human_status: MatrixBotDmRoomStatus::Invited,
        invite_error: None,
    });

    let report = run_matrix_client_bridge_once(&config, client.clone()).expect("run client bridge");

    let requests = agentd.requests();
    agentd.join();
    assert_eq!(report.run.registered_rooms, 1);
    assert_eq!(report.run.inbound_forwarded, 0);
    assert_eq!(report.run.outbound_sent, 0);
    assert_eq!(report.run.bot_command_replies_sent, 1);
    assert_eq!(
        client.invited_users(),
        [(
            "!codex-worker:matrix.test".to_owned(),
            "@alex:matrix.test".to_owned(),
        )]
    );
    assert_eq!(client.sent().len(), 1);
    assert!(
        client.sent()[0]
            .1
            .contains("DM room ready for codex-worker")
    );
    assert_eq!(requests[0].path, "/api/runtime/capabilities");
    assert_eq!(
        requests[1].path,
        "/api/matrix/outbox/cursor?bridgeId=matrix-bridge"
    );
    assert_eq!(requests[2].path, "/api/agents");
    assert_eq!(requests[3].path, "/api/groups");
    assert_eq!(requests[4].method, "GET");
    assert_eq!(requests[4].path, "/api/agents/codex-worker");
    assert_eq!(requests[5].method, "POST");
    assert_eq!(requests[5].path, "/api/matrix/rooms");
    assert_eq!(requests[6].method, "GET");
    assert_eq!(requests[6].path, "/api/matrix/outbox?from_seq=0");
    let state = read_json(&state_path);
    assert_eq!(state["nextFromSeq"], 0);
}

#[test]
fn matrix_client_bridge_once_executes_dm_lifecycle_with_fake_client() {
    let agentd = FakeHttpServer::new(vec![
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
        FakeResponse::status(200, json!([]).to_string()),
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
        outbox_response(Vec::new()),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let config = bridge_config(&agentd, state_path);
    let client = SharedFakeMatrixClient::new(MatrixClientSync {
        invites: Vec::new(),
        joined_rooms: Vec::new(),
        text_events: vec![command_text("!dm codex-worker")],
    });
    client.state.lock().expect("fake state").next_direct_room_id =
        Some("!dm-created:matrix.test".to_owned());

    let report = run_matrix_client_bridge_once(&config, client.clone()).expect("run client bridge");

    let requests = agentd.requests();
    agentd.join();
    assert_eq!(report.run.bot_command_replies_sent, 1);
    assert_eq!(
        client.created_direct_rooms(),
        [(
            "DM: codex-worker".to_owned(),
            vec![
                "@alex:matrix.test".to_owned(),
                "@ac_codex-worker:matrix.test".to_owned(),
            ],
        )]
    );
    assert_eq!(client.sent().len(), 1);
    assert!(client.sent()[0].1.contains("DM room ready"));
    assert_eq!(requests[0].path, "/api/runtime/capabilities");
    assert_eq!(requests[2].path, "/api/agents");
}

#[test]
fn matrix_client_bridge_once_runner_provisions_puppets_before_matrix_sync() {
    let homeserver = FakeHttpServer::new(vec![matrix_login_response()]);
    let agentd = FakeHttpServer::new(vec![
        native_caps_response(),
        cursor_response(),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        outbox_response(Vec::new()),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let puppet_state_path = dir.path().join("bridge-state.json");
    let mut config = bridge_config(&agentd, state_path);
    config.puppet_accounts = Some(puppet_accounts(&homeserver, &puppet_state_path));
    let client = SharedFakeMatrixClient::new(sync_with_direct_room(Vec::new()))
        .with_required_agent_token(&puppet_state_path, "codex-worker", "codex-token");

    let report = run_matrix_client_bridge_once(&config, client.clone()).expect("run client bridge");

    let homeserver_requests = homeserver.requests();
    let agentd_requests = agentd.requests();
    homeserver.join();
    agentd.join();
    let puppet_report = report
        .puppet_account_provisioning
        .as_ref()
        .expect("puppet report");
    assert_eq!(
        puppet_report.outcomes(),
        &[MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(client.calls()[0..2], ["ensure_logged_in", "sync_once"]);
    assert_eq!(homeserver_requests[0].method, "POST");
    assert_eq!(homeserver_requests[0].path, "/_matrix/client/v3/login");
    assert_eq!(agentd_requests[0].method, "GET");
    assert_eq!(agentd_requests[0].path, "/api/runtime/capabilities");
    assert_eq!(agentd_requests[2].method, "POST");
    assert_eq!(agentd_requests[2].path, "/api/matrix/rooms");
    let puppet_state = read_json(&puppet_state_path);
    assert_eq!(puppet_state["agentTokens"]["codex-worker"], "codex-token");
}

#[test]
fn matrix_client_bridge_once_runner_preserves_cursor_on_matrix_send_failure() {
    let agentd = FakeHttpServer::new(vec![
        native_caps_response(),
        cursor_response(),
        FakeResponse::status(200, json!({"ok": true}).to_string()),
        outbox_response(vec![(21, "first"), (22, "second")]),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    write_json(&state_path, &json!({"nextFromSeq": 20}));
    let config = bridge_config(&agentd, state_path.clone());
    let client = SharedFakeMatrixClient::new(sync_with_direct_room(Vec::new()))
        .with_fail_send_body("second");

    let err = run_matrix_client_bridge_once(&config, client.clone())
        .expect_err("second Matrix send fails");

    agentd.join();
    assert!(matches!(err, BridgeError::Transport(_)));
    assert_eq!(
        client.sent(),
        vec![("!codex-worker:matrix.test".to_owned(), "first".to_owned())]
    );
    let state = read_json(&state_path);
    assert_eq!(state["nextFromSeq"], 20);
}
