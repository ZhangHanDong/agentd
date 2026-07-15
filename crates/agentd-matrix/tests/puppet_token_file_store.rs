use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    BridgeError, MatrixPuppetAccountOutcome, MatrixPuppetDirectory, MatrixPuppetHttpAccountConfig,
    MatrixPuppetHttpAccountProvisioner, MatrixPuppetProvisioningConfig, MatrixPuppetTokenFileStore,
    MatrixPuppetTokenSink,
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
struct FakeHomeserver {
    base_url: String,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    handle: thread::JoinHandle<()>,
}

impl FakeHomeserver {
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

    fn base_url(&self) -> String {
        self.base_url.clone()
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
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create parent dir");
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).expect("encode JSON"),
    )
    .expect("write JSON");
}

fn read_json(path: &Path) -> Value {
    let contents = std::fs::read_to_string(path).expect("read JSON");
    serde_json::from_str(&contents).expect("decode JSON")
}

fn directory(agent_names: &[&str]) -> MatrixPuppetDirectory {
    MatrixPuppetDirectory::new("matrix.test", "ac_", agent_names, Vec::<&str>::new())
        .expect("puppet directory")
}

fn secret_config() -> MatrixPuppetProvisioningConfig {
    MatrixPuppetProvisioningConfig {
        password_secret: Some("matrix-secret".to_owned()),
        ..MatrixPuppetProvisioningConfig::default()
    }
}

#[test]
fn matrix_puppet_token_file_store_loads_agent_chat_state_and_preserves_unknown_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bridge-state.json");
    write_json(
        &path,
        &json!({
            "botToken": "bot-token",
            "agentTokens": {
                "CODEX-WORKER": "old-token",
                "reviewer": "review-token"
            },
            "roomGroupMap": {"!ops:matrix.test": "ops"},
            "customField": {"preserve": true}
        }),
    );
    let mut store = MatrixPuppetTokenFileStore::new(&path);

    let token_state = store.load_token_state().expect("load token state");
    assert_eq!(
        token_state.token_name_for_agent("codex-worker"),
        Some("CODEX-WORKER")
    );
    assert_eq!(
        token_state.token_for_agent("CODEX-WORKER"),
        Some("old-token")
    );

    store
        .save_agent_token("codex-worker", "new-token")
        .expect("save replacement token");

    let value = read_json(&path);
    assert_eq!(value["agentTokens"]["CODEX-WORKER"], "new-token");
    assert!(value["agentTokens"].get("codex-worker").is_none());
    assert_eq!(value["agentTokens"]["reviewer"], "review-token");
    assert_eq!(value["botToken"], "bot-token");
    assert_eq!(value["roomGroupMap"]["!ops:matrix.test"], "ops");
    assert_eq!(value["customField"]["preserve"], true);
}

#[test]
fn matrix_puppet_token_file_store_creates_missing_state_and_parent_dirs() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("nested").join("bridge-state.json");
    let mut store = MatrixPuppetTokenFileStore::new(&path);

    let token_state = store.load_token_state().expect("missing state loads");
    assert_eq!(token_state.token_for_agent("codex-worker"), None);

    store
        .save_agent_token("codex-worker", "created-token")
        .expect("save first token");

    let value = read_json(&path);
    assert_eq!(value["agentTokens"]["codex-worker"], "created-token");
    assert!(
        value["agentTokens"]
            .as_object()
            .expect("agentTokens object")
            .len()
            == 1
    );
}

#[test]
fn matrix_puppet_token_file_store_deletes_stale_tokens_without_touching_other_state() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bridge-state.json");
    write_json(
        &path,
        &json!({
            "botToken": "bot-token",
            "agentTokens": {
                "codex-worker": "codex-token",
                "old-agent": "stale-token"
            },
            "groupRoomMap": {"ops": "!ops:matrix.test"}
        }),
    );
    let mut store = MatrixPuppetTokenFileStore::new(&path);

    store
        .delete_agent_token("old-agent")
        .expect("delete stale token");

    let value = read_json(&path);
    assert!(value["agentTokens"].get("old-agent").is_none());
    assert_eq!(value["agentTokens"]["codex-worker"], "codex-token");
    assert_eq!(value["botToken"], "bot-token");
    assert_eq!(value["groupRoomMap"]["ops"], "!ops:matrix.test");
}

#[test]
fn matrix_puppet_token_file_store_rejects_malformed_json_without_overwriting() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bridge-state.json");
    std::fs::write(&path, "{not valid json").expect("write malformed JSON");
    let mut store = MatrixPuppetTokenFileStore::new(&path);

    assert!(matches!(
        store.load_token_state(),
        Err(BridgeError::State(_))
    ));
    assert!(matches!(
        store.save_agent_token("codex-worker", "new-token"),
        Err(BridgeError::State(_))
    ));
    assert_eq!(
        std::fs::read_to_string(&path).expect("read malformed JSON"),
        "{not valid json"
    );
}

#[test]
fn matrix_puppet_token_file_store_persists_http_provisioner_updates() {
    let server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({
            "user_id": "@ac_codex-worker:matrix.test",
            "access_token": "codex-token"
        })
        .to_string(),
    )]);
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("bridge-state.json");
    write_json(
        &path,
        &json!({
            "botToken": "bot-token",
            "agentTokens": {"old-agent": "stale-token"},
            "roomGroupMap": {"!ops:matrix.test": "ops"}
        }),
    );
    let http_config = MatrixPuppetHttpAccountConfig::new(server.base_url()).expect("http config");
    let provisioner =
        MatrixPuppetHttpAccountProvisioner::new(&http_config).expect("account provisioner");
    let mut store = MatrixPuppetTokenFileStore::new(&path);
    let token_state = store.load_token_state().expect("load token state");

    let report = provisioner.provision(
        &directory(&["codex-worker"]),
        &secret_config(),
        &token_state,
        &mut store,
    );

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/_matrix/client/v3/login");
    let request_body: Value = serde_json::from_str(&requests[0].body).expect("login body");
    assert_eq!(request_body["identifier"]["user"], "ac_codex-worker");
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(report.pruned_token_names(), &["old-agent".to_owned()]);

    let value = read_json(&path);
    assert_eq!(value["agentTokens"]["codex-worker"], "codex-token");
    assert!(value["agentTokens"].get("old-agent").is_none());
    assert_eq!(value["botToken"], "bot-token");
    assert_eq!(value["roomGroupMap"]["!ops:matrix.test"], "ops");
}
