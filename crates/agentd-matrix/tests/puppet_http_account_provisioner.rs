use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};
use std::thread;

use agentd_matrix::{
    BridgeError, MatrixPuppetAccountOutcome, MatrixPuppetAccountStep, MatrixPuppetDirectory,
    MatrixPuppetHttpAccountConfig, MatrixPuppetHttpAccountProvisioner,
    MatrixPuppetProvisioningConfig, MatrixPuppetTokenSink, MatrixPuppetTokenState,
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

#[derive(Debug, Default)]
struct FakeTokenSink {
    saved: BTreeMap<String, String>,
    deleted: Vec<String>,
}

impl MatrixPuppetTokenSink for FakeTokenSink {
    fn save_agent_token(
        &mut self,
        agent_name: &str,
        access_token: &str,
    ) -> Result<(), BridgeError> {
        self.saved
            .insert(agent_name.to_owned(), access_token.to_owned());
        Ok(())
    }

    fn delete_agent_token(&mut self, token_name: &str) -> Result<(), BridgeError> {
        self.deleted.push(token_name.to_owned());
        Ok(())
    }
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

fn provisioner(server: &FakeHomeserver) -> MatrixPuppetHttpAccountProvisioner {
    let config = MatrixPuppetHttpAccountConfig::new(server.base_url()).expect("account config");
    MatrixPuppetHttpAccountProvisioner::new(&config).expect("account provisioner")
}

#[test]
fn matrix_puppet_http_account_provisioner_reuses_valid_tokens_and_prunes_stale() {
    let server = FakeHomeserver::new(vec![FakeResponse::status(
        200,
        json!({"user_id": "@ac_codex-worker:matrix.test"}).to_string(),
    )]);
    let token_state = MatrixPuppetTokenState::from_agent_tokens([
        ("CODEX-WORKER", "existing-token"),
        ("old-agent", "stale-token"),
    ]);
    let mut sink = FakeTokenSink::default();

    let report = provisioner(&server).provision(
        &directory(&["codex-worker"]),
        &MatrixPuppetProvisioningConfig::default(),
        &token_state,
        &mut sink,
    );

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].method, "GET");
    assert_eq!(requests[0].path, "/_matrix/client/v3/account/whoami");
    assert_eq!(
        requests[0].header("authorization"),
        Some("Bearer existing-token")
    );
    assert_eq!(
        report.outcomes(),
        &[MatrixPuppetAccountOutcome::ReusedToken {
            agent_name: "codex-worker".to_owned(),
            localpart: "ac_codex-worker".to_owned(),
            mxid: "@ac_codex-worker:matrix.test".to_owned(),
            token_name: "CODEX-WORKER".to_owned(),
            user_id: "@ac_codex-worker:matrix.test".to_owned(),
        }]
    );
    assert_eq!(report.pruned_token_names(), &["old-agent".to_owned()]);
    assert!(report.prune_failures().is_empty());
    assert_eq!(sink.deleted, ["old-agent".to_owned()]);
    assert!(sink.saved.is_empty());
}

#[test]
fn matrix_puppet_http_account_provisioner_logs_in_and_registers_via_http() {
    let server = FakeHomeserver::new(vec![
        FakeResponse::status(
            200,
            json!({
                "user_id": "@ac_codex-worker:matrix.test",
                "access_token": "codex-token"
            })
            .to_string(),
        ),
        FakeResponse::status(403, json!({"errcode": "M_FORBIDDEN"}).to_string()),
        FakeResponse::status(
            401,
            json!({
                "session": "review-session",
                "flows": [{"stages": ["m.login.registration_token"]}]
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "user_id": "@ac_reviewer:matrix.test",
                "access_token": "reviewer-token"
            })
            .to_string(),
        ),
    ]);
    let http_config = MatrixPuppetHttpAccountConfig::new(server.base_url())
        .expect("account config")
        .with_registration_token("registration-token");
    let provisioner =
        MatrixPuppetHttpAccountProvisioner::new(&http_config).expect("account provisioner");
    let mut sink = FakeTokenSink::default();

    let report = provisioner.provision(
        &directory(&["codex-worker", "reviewer"]),
        &secret_config(),
        &MatrixPuppetTokenState::default(),
        &mut sink,
    );

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 4);
    assert_eq!(requests[0].path, "/_matrix/client/v3/login");
    assert_eq!(
        requests[0].json_body()["identifier"]["user"],
        "ac_codex-worker"
    );
    assert_eq!(requests[1].path, "/_matrix/client/v3/login");
    assert_eq!(requests[1].json_body()["identifier"]["user"], "ac_reviewer");
    assert_eq!(requests[2].path, "/_matrix/client/v3/register");
    assert_eq!(requests[2].json_body()["username"], "ac_reviewer");
    assert_eq!(requests[3].path, "/_matrix/client/v3/register");
    let completion = requests[3].json_body();
    assert_eq!(completion["username"], "ac_reviewer");
    assert_eq!(completion["auth"]["type"], "m.login.registration_token");
    assert_eq!(completion["auth"]["token"], "registration-token");
    assert_eq!(completion["auth"]["session"], "review-session");
    assert_eq!(
        report.outcomes(),
        &[
            MatrixPuppetAccountOutcome::LoggedIn {
                agent_name: "codex-worker".to_owned(),
                localpart: "ac_codex-worker".to_owned(),
                mxid: "@ac_codex-worker:matrix.test".to_owned(),
                user_id: "@ac_codex-worker:matrix.test".to_owned(),
            },
            MatrixPuppetAccountOutcome::Registered {
                agent_name: "reviewer".to_owned(),
                localpart: "ac_reviewer".to_owned(),
                mxid: "@ac_reviewer:matrix.test".to_owned(),
                user_id: "@ac_reviewer:matrix.test".to_owned(),
            },
        ]
    );
    assert_eq!(
        sink.saved.get("codex-worker"),
        Some(&"codex-token".to_owned())
    );
    assert_eq!(
        sink.saved.get("reviewer"),
        Some(&"reviewer-token".to_owned())
    );
}

#[test]
fn matrix_puppet_http_account_provisioner_reports_http_errors_without_stopping() {
    let server = FakeHomeserver::new(vec![
        FakeResponse::status(403, json!({"errcode": "M_FORBIDDEN"}).to_string()),
        FakeResponse::status(
            401,
            json!({
                "session": "broken-session",
                "flows": [{"stages": ["m.login.email.identity"]}]
            })
            .to_string(),
        ),
        FakeResponse::status(
            200,
            json!({
                "user_id": "@ac_beta:matrix.test",
                "access_token": "beta-token"
            })
            .to_string(),
        ),
    ]);
    let mut sink = FakeTokenSink::default();

    let report = provisioner(&server).provision(
        &directory(&["alpha", "beta"]),
        &secret_config(),
        &MatrixPuppetTokenState::default(),
        &mut sink,
    );

    let requests = server.requests();
    server.join();
    assert_eq!(requests.len(), 3);
    assert_eq!(requests[0].path, "/_matrix/client/v3/login");
    assert_eq!(requests[1].path, "/_matrix/client/v3/register");
    assert_eq!(requests[2].path, "/_matrix/client/v3/login");
    assert_eq!(report.outcomes().len(), 2);
    match &report.outcomes()[0] {
        MatrixPuppetAccountOutcome::Failed {
            agent_name,
            localpart,
            step,
            error,
            ..
        } => {
            assert_eq!(agent_name, "alpha");
            assert_eq!(localpart, "ac_alpha");
            assert_eq!(step, &MatrixPuppetAccountStep::Register);
            assert!(
                error.contains("No usable Matrix registration flow"),
                "unexpected error: {error}"
            );
        }
        other => panic!("expected failed outcome, got {other:?}"),
    }
    assert_eq!(
        report.outcomes()[1],
        MatrixPuppetAccountOutcome::LoggedIn {
            agent_name: "beta".to_owned(),
            localpart: "ac_beta".to_owned(),
            mxid: "@ac_beta:matrix.test".to_owned(),
            user_id: "@ac_beta:matrix.test".to_owned(),
        }
    );
    assert_eq!(sink.saved.len(), 1);
    assert_eq!(sink.saved.get("beta"), Some(&"beta-token".to_owned()));
    assert!(!sink.saved.contains_key("alpha"));
}
