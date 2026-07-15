use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_bin::matrix_bridge::run_matrix_client_bridge_preflight;
use agentd_bin::{DaemonConfig, MatrixClientBridgeServiceArgs};
use agentd_matrix::BridgeError;
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
struct FakeMatrixHomeserver {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: thread::JoinHandle<()>,
}

impl FakeMatrixHomeserver {
    fn new(responses: Vec<FakeResponse>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake homeserver");
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
        self.handle.join().expect("fake homeserver thread");
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

fn daemon_config(api_token: Option<&str>) -> DaemonConfig {
    DaemonConfig {
        db_path: PathBuf::from("agentd.db"),
        port: 8787,
        workflows_dir: PathBuf::from("workflows"),
        repo_dir: PathBuf::from("."),
        worktree_base: PathBuf::from(".agentd/worktrees"),
        accept_workflow_change: false,
        log_level: "info".to_owned(),
        api_token: api_token.map(ToOwned::to_owned),
        agent_tokens: Vec::new(),
        agent_token_mode: "audit".to_owned(),
    }
}

fn preflight_args(
    homeserver_url: Option<String>,
    state_path: PathBuf,
    puppet_state_path: Option<PathBuf>,
) -> MatrixClientBridgeServiceArgs {
    MatrixClientBridgeServiceArgs {
        agentd_api: "http://127.0.0.1:8787".to_owned(),
        state: state_path,
        iterations: 1,
        matrix_homeserver_url: homeserver_url,
        matrix_username: None,
        matrix_password: None,
        matrix_user_id: Some("@agentd-bot:matrix.test".to_owned()),
        matrix_device_id: Some("DEVICE".to_owned()),
        matrix_access_token: Some("bot-token".to_owned()),
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
        matrix_puppet_state: puppet_state_path,
        matrix_agent_password_secret: Some("matrix-secret".to_owned()),
        matrix_agent_password_template: None,
        matrix_allow_legacy_agent_password: false,
        matrix_registration_token: Some("registration-token".to_owned()),
    }
}

#[test]
fn agentd_bin_matrix_client_bridge_preflight_probes_versions_and_whoami_without_state_mutation() {
    let homeserver = FakeMatrixHomeserver::new(vec![
        FakeResponse::status(
            200,
            json!({"versions": ["v1.12", "v1.11"], "unstable_features": {}}).to_string(),
        ),
        FakeResponse::status(
            200,
            json!({"user_id": "@agentd-bot:matrix.test"}).to_string(),
        ),
    ]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let puppet_state_path = dir.path().join("bridge-state.json");
    let args = preflight_args(
        Some(homeserver.base_url().to_owned()),
        state_path.clone(),
        Some(puppet_state_path.clone()),
    );

    let report = run_matrix_client_bridge_preflight(&daemon_config(Some("bridge-secret")), &args)
        .expect("preflight");
    let homeserver_url = homeserver.base_url().to_owned();

    let requests = homeserver.requests();
    homeserver.join();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/_matrix/client/versions");
    assert!(requests[0].body.is_empty());
    assert_eq!(requests[1].method, "GET");
    assert_eq!(requests[1].path, "/_matrix/client/v3/account/whoami");
    assert_eq!(
        requests[1].header("Authorization"),
        Some("Bearer bot-token")
    );
    assert!(requests[1].body.is_empty());
    assert_eq!(report.iterations, 1);
    assert!(report.puppet_accounts_configured);
    assert_eq!(report.homeserver.homeserver_url, homeserver_url);
    assert_eq!(
        report.homeserver.versions,
        vec!["v1.12".to_owned(), "v1.11".to_owned()]
    );
    assert_eq!(
        report.homeserver.whoami_user_id.as_deref(),
        Some("@agentd-bot:matrix.test")
    );
    assert!(
        !state_path.exists(),
        "preflight must not create bridge cursor state"
    );
    assert!(
        !puppet_state_path.exists(),
        "preflight must not create puppet token state"
    );
}

#[test]
fn agentd_bin_matrix_client_bridge_preflight_rejects_malformed_versions_without_state_mutation() {
    let homeserver = FakeMatrixHomeserver::new(vec![FakeResponse::status(200, "not-json")]);
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let puppet_state_path = dir.path().join("bridge-state.json");
    let args = preflight_args(
        Some(homeserver.base_url().to_owned()),
        state_path.clone(),
        Some(puppet_state_path.clone()),
    );

    let err = run_matrix_client_bridge_preflight(&daemon_config(None), &args)
        .expect_err("malformed versions fail");

    let requests = homeserver.requests();
    homeserver.join();
    assert!(matches!(err, BridgeError::Transport(_)));
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].path, "/_matrix/client/versions");
    assert!(
        !state_path.exists(),
        "preflight must not create bridge cursor state on failure"
    );
    assert!(
        !puppet_state_path.exists(),
        "preflight must not create puppet token state on failure"
    );
}

#[test]
fn agentd_bin_matrix_client_bridge_preflight_requires_homeserver_url() {
    let dir = tempfile::tempdir().expect("tempdir");
    let state_path = dir.path().join("state.json");
    let mut args = preflight_args(None, state_path.clone(), None);
    args.matrix_access_token = None;
    args.matrix_agent_password_secret = None;
    args.matrix_registration_token = None;

    let err = run_matrix_client_bridge_preflight(&daemon_config(None), &args)
        .expect_err("missing homeserver URL fails");

    assert!(matches!(err, BridgeError::InvalidConfig(_)));
    assert!(
        !state_path.exists(),
        "preflight must not create bridge cursor state"
    );
}
