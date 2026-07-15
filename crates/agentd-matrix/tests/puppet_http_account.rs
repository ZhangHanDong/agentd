use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    BridgeError, MatrixPuppetAccountPort, MatrixPuppetHttpAccountConfig,
    MatrixPuppetHttpAccountPort,
};
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

    fn json_body(&self) -> Value {
        serde_json::from_str(&self.body).expect("request JSON body")
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

fn account_port(server: &FakeHomeserver) -> MatrixPuppetHttpAccountPort {
    let config = MatrixPuppetHttpAccountConfig::new(server.base_url()).expect("account config");
    MatrixPuppetHttpAccountPort::new(&config).expect("account port")
}

#[test]
fn matrix_puppet_http_account_port_whoami_sends_bearer_and_reads_user_id() {
    let server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({"user_id": "@ac_codex-worker:matrix.test"}).to_string(),
    )]);
    let mut port = account_port(&server);

    let whoami = port.whoami("existing-token").expect("whoami");

    let requests = server.requests();
    server.join();
    assert_eq!(whoami.user_id, "@ac_codex-worker:matrix.test");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/_matrix/client/v3/account/whoami");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer existing-token")
    );
}

#[test]
fn matrix_puppet_http_account_port_login_posts_password_identifier_json() {
    let server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({
            "user_id": "@ac_codex-worker:matrix.test",
            "access_token": "login-token"
        })
        .to_string(),
    )]);
    let mut port = account_port(&server);

    let session = port
        .login("ac_codex-worker", "agent-password")
        .expect("login");

    let requests = server.requests();
    server.join();
    assert_eq!(session.user_id, "@ac_codex-worker:matrix.test");
    assert_eq!(session.access_token, "login-token");
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/_matrix/client/v3/login");
    let body = requests[0].json_body();
    assert_eq!(body["type"], "m.login.password");
    assert_eq!(body["identifier"]["type"], "m.id.user");
    assert_eq!(body["identifier"]["user"], "ac_codex-worker");
    assert_eq!(body["password"], "agent-password");
}

#[test]
fn matrix_puppet_http_account_port_register_accepts_access_token_probe() {
    let server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({
            "user_id": "@ac_codex-worker:matrix.test",
            "access_token": "probe-token"
        })
        .to_string(),
    )]);
    let mut port = account_port(&server);

    let session = port
        .register("ac_codex-worker", "agent-password")
        .expect("register");

    let requests = server.requests();
    server.join();
    assert_eq!(session.user_id, "@ac_codex-worker:matrix.test");
    assert_eq!(session.access_token, "probe-token");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "POST");
    assert_eq!(requests[0].path, "/_matrix/client/v3/register");
    let body = requests[0].json_body();
    assert_eq!(body["username"], "ac_codex-worker");
    assert_eq!(body["password"], "agent-password");
    assert!(body.get("auth").is_none());
}

#[test]
fn matrix_puppet_http_account_port_register_completes_token_or_dummy_uia() {
    let dummy_server = FakeHomeserver::new(vec![
        FakeResponse::status(
            401,
            json!({
                "session": "dummy-session",
                "flows": [{"stages": ["m.login.dummy"]}]
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "user_id": "@ac_codex-worker:matrix.test",
                "access_token": "dummy-token"
            })
            .to_string(),
        ),
    ]);
    let mut dummy_port = account_port(&dummy_server);

    let dummy_session = dummy_port
        .register("ac_codex-worker", "agent-password")
        .expect("dummy register");

    let dummy_requests = dummy_server.requests();
    dummy_server.join();
    assert_eq!(dummy_session.access_token, "dummy-token");
    assert_eq!(dummy_requests.len(), 2);
    assert_eq!(dummy_requests[1].path, "/_matrix/client/v3/register");
    let dummy_body = dummy_requests[1].json_body();
    assert_eq!(dummy_body["username"], "ac_codex-worker");
    assert_eq!(dummy_body["password"], "agent-password");
    assert_eq!(dummy_body["auth"]["type"], "m.login.dummy");
    assert_eq!(dummy_body["auth"]["session"], "dummy-session");

    let token_server = FakeHomeserver::new(vec![
        FakeResponse::status(
            401,
            json!({
                "session": "token-session",
                "flows": [{"stages": ["m.login.registration_token"]}]
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "user_id": "@ac_codex-reviewer:matrix.test",
                "access_token": "token-register-token"
            })
            .to_string(),
        ),
    ]);
    let token_config = MatrixPuppetHttpAccountConfig::new(token_server.base_url())
        .expect("token config")
        .with_registration_token("reg-token");
    let mut token_port = MatrixPuppetHttpAccountPort::new(&token_config).expect("token port");

    let token_session = token_port
        .register("ac_codex-reviewer", "agent-password")
        .expect("token register");

    let token_requests = token_server.requests();
    token_server.join();
    assert_eq!(token_session.access_token, "token-register-token");
    assert_eq!(token_requests.len(), 2);
    let token_body = token_requests[1].json_body();
    assert_eq!(token_body["auth"]["type"], "m.login.registration_token");
    assert_eq!(token_body["auth"]["token"], "reg-token");
    assert_eq!(token_body["auth"]["session"], "token-session");

    assert_eq!(dummy_body["auth"]["session"], "dummy-session");
}

#[test]
fn matrix_puppet_http_account_port_reports_status_malformed_and_no_uia_errors() {
    let status_server = FakeHomeserver::new(vec![FakeResponse::status(
        403,
        json!({"errcode": "M_FORBIDDEN"}).to_string(),
    )]);
    let mut status_port = account_port(&status_server);
    assert_transport(status_port.login("ac_codex-worker", "bad-password"));
    status_server.join();

    let malformed_server = FakeHomeserver::new(vec![FakeResponse::status(200, "not json")]);
    let mut malformed_port = account_port(&malformed_server);
    assert_transport(malformed_port.whoami("bad-token"));
    malformed_server.join();

    let missing_server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({"user_id": "@u:s"}).to_string(),
    )]);
    let mut missing_port = account_port(&missing_server);
    assert_transport(missing_port.login("ac_codex-worker", "password"));
    missing_server.join();

    let no_flow_server = FakeHomeserver::new(vec![FakeResponse::status(
        401,
        json!({
            "session": "no-flow-session",
            "flows": [{"stages": ["m.login.sso"]}]
        })
        .to_string(),
    )]);
    let mut no_flow_port = account_port(&no_flow_server);
    assert_transport(no_flow_port.register("ac_codex-worker", "password"));
    no_flow_server.join();
}

fn assert_transport<T: std::fmt::Debug>(result: Result<T, BridgeError>) {
    let err = result.expect_err("transport error");
    assert!(
        matches!(err, BridgeError::Transport(_)),
        "expected Matrix transport error, got {err}"
    );
}
