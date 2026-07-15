use std::io::{Read, Write};
use std::process::{Command, Output};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn agentctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .args(args)
        .output()
        .expect("spawn agentctl")
}

fn agentctl_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_agentctl"));
    cmd.args(args);
    for (key, value) in envs {
        cmd.env(key, value);
    }
    cmd.output().expect("spawn agentctl")
}

fn read_http_request(sock: &mut std::net::TcpStream) -> String {
    sock.set_read_timeout(Some(Duration::from_secs(1)))
        .expect("set read timeout");
    let start = Instant::now();
    let mut raw = Vec::new();
    let mut buf = [0_u8; 8192];
    while start.elapsed() < Duration::from_secs(1) {
        match sock.read(&mut buf) {
            Ok(0) if raw.is_empty() => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(0) => break,
            Ok(n) => {
                raw.extend_from_slice(&buf[..n]);
                if raw.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            Err(e)
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) && raw.is_empty() =>
            {
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&raw).to_string()
}

fn start_fake_daemon(responses: Vec<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_thread = Arc::clone(&seen);
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut responses = responses.into_iter();
        while Instant::now() < deadline {
            let Some(body) = responses.next() else {
                break;
            };
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let raw = read_http_request(&mut sock);
                    let request_line = raw.lines().next().unwrap_or("").to_string();
                    seen_thread.lock().expect("seen lock").push(request_line);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    responses = std::iter::once(body)
                        .chain(responses)
                        .collect::<Vec<_>>()
                        .into_iter();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

fn start_fake_daemon_raw(responses: Vec<&'static str>) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("nonblocking");
    let port = listener.local_addr().expect("addr").port();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_thread = Arc::clone(&seen);
    std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut responses = responses.into_iter();
        while Instant::now() < deadline {
            let Some(body) = responses.next() else {
                break;
            };
            match listener.accept() {
                Ok((mut sock, _)) => {
                    let raw = read_http_request(&mut sock);
                    seen_thread.lock().expect("seen lock").push(raw);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    let _ = sock.write_all(resp.as_bytes());
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    responses = std::iter::once(body)
                        .chain(responses)
                        .collect::<Vec<_>>()
                        .into_iter();
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => break,
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), seen)
}

#[test]
fn agent_cli_auth_headers_use_flags_and_env() {
    let (url, seen) = start_fake_daemon_raw(vec![
        "[]",
        r#"{"runtimeProfile":{}}"#,
        r#"{"ok":true,"runtime":{"agent":"codex-sec"}}"#,
    ]);

    let ls = agentctl(&[
        "agent",
        "ls",
        "--api-token",
        "operator-secret",
        "--daemon-url",
        &url,
    ]);
    assert!(
        ls.status.success(),
        "ls stderr: {}",
        String::from_utf8_lossy(&ls.stderr)
    );

    let launch_env = agentctl_env(
        &["agent", "launch-env", "codex-sec", "--daemon-url", &url],
        &[("AGENTD_API_TOKEN", "env-operator-secret")],
    );
    assert!(
        launch_env.status.success(),
        "launch-env stderr: {}",
        String::from_utf8_lossy(&launch_env.stderr)
    );

    let runtime = agentctl(&[
        "agent",
        "runtime",
        "codex-sec",
        "--blocked",
        "--agent-token",
        "agent-secret",
        "--daemon-url",
        &url,
    ]);
    assert!(
        runtime.status.success(),
        "runtime stderr: {}",
        String::from_utf8_lossy(&runtime.stderr)
    );

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(seen.len(), 3, "three requests: {seen:?}");
    assert!(
        seen[0].contains("Authorization: Bearer operator-secret"),
        "operator flag header: {}",
        seen[0]
    );
    assert!(
        seen[1].contains("Authorization: Bearer env-operator-secret"),
        "operator env header: {}",
        seen[1]
    );
    assert!(
        seen[2].contains("X-Agent-Token: agent-secret"),
        "agent token header: {}",
        seen[2]
    );
}

#[test]
fn agent_cli_register_ls_inspect_heartbeat_and_offline_use_api_agents() {
    let (url, seen) = start_fake_daemon(vec![
        r#"{"ok":true,"agent":{"name":"codex-sec","status":"online"}}"#,
        r#"[{"name":"codex-sec","status":"online"}]"#,
        r#"{"name":"codex-sec","status":"online","runtime":"codex"}"#,
        r#"{"ok":true,"created":false,"agent":{"name":"codex-sec","status":"online"}}"#,
        r#"{"ok":true,"agent":{"name":"codex-sec","status":"offline"}}"#,
    ]);

    let register = agentctl(&[
        "agent",
        "register",
        "codex-sec",
        "--role",
        "reviewer",
        "--capability",
        "strong",
        "--runtime",
        "codex",
        "--model",
        "gpt-5",
        "--tmux-target",
        "codex-sec:0.0",
        "--daemon-url",
        &url,
    ]);
    assert!(
        register.status.success(),
        "register stderr: {}",
        String::from_utf8_lossy(&register.stderr)
    );
    assert!(
        String::from_utf8_lossy(&register.stdout).contains("codex-sec"),
        "register prints daemon body"
    );

    let ls = agentctl(&["agent", "ls", "--daemon-url", &url]);
    assert!(
        ls.status.success(),
        "ls stderr: {}",
        String::from_utf8_lossy(&ls.stderr)
    );
    assert!(String::from_utf8_lossy(&ls.stdout).contains("codex-sec"));

    let inspect = agentctl(&["agent", "inspect", "codex-sec", "--daemon-url", &url]);
    assert!(
        inspect.status.success(),
        "inspect stderr: {}",
        String::from_utf8_lossy(&inspect.stderr)
    );
    assert!(String::from_utf8_lossy(&inspect.stdout).contains("codex"));

    let heartbeat = agentctl(&[
        "agent",
        "heartbeat",
        "codex-sec",
        "--server",
        "local",
        "--tmux-target",
        "codex-sec:0.0",
        "--daemon-url",
        &url,
    ]);
    assert!(
        heartbeat.status.success(),
        "heartbeat stderr: {}",
        String::from_utf8_lossy(&heartbeat.stderr)
    );
    assert!(String::from_utf8_lossy(&heartbeat.stdout).contains("created"));

    let offline = agentctl(&[
        "agent",
        "offline",
        "codex-sec",
        "--reason",
        "manual-offline",
        "--daemon-url",
        &url,
    ]);
    assert!(
        offline.status.success(),
        "offline stderr: {}",
        String::from_utf8_lossy(&offline.stderr)
    );
    assert!(String::from_utf8_lossy(&offline.stdout).contains("offline"));

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(
        seen,
        vec![
            "POST /api/agents HTTP/1.1",
            "GET /api/agents HTTP/1.1",
            "GET /api/agents/codex-sec HTTP/1.1",
            "POST /api/agents/codex-sec/heartbeat HTTP/1.1",
            "POST /api/agents/codex-sec/offline HTTP/1.1",
        ]
    );
}

#[test]
fn agent_cli_launch_env_start_and_runtime_use_api_agents() {
    let (url, seen) = start_fake_daemon(vec![
        r#"{"runtimeProfile":{"primary":{"framework":"codex","model":"gpt-5"}}}"#,
        r#"{"ok":true,"agent":{"name":"codex-sec","status":"online"},"handle":{"address":"fake://codex-sec"}}"#,
        r#"{"ok":true,"runtime":{"agent":"codex-sec","blocked":true}}"#,
    ]);

    let launch_env = agentctl(&["agent", "launch-env", "codex-sec", "--daemon-url", &url]);
    assert!(
        launch_env.status.success(),
        "launch-env stderr: {}",
        String::from_utf8_lossy(&launch_env.stderr)
    );
    assert!(String::from_utf8_lossy(&launch_env.stdout).contains("runtimeProfile"));

    let start = agentctl(&["agent", "start", "codex-sec", "--daemon-url", &url]);
    assert!(
        start.status.success(),
        "start stderr: {}",
        String::from_utf8_lossy(&start.stderr)
    );
    assert!(String::from_utf8_lossy(&start.stdout).contains("fake://codex-sec"));

    let runtime = agentctl(&[
        "agent",
        "runtime",
        "codex-sec",
        "--blocked",
        "--reason",
        "waiting for reviewer",
        "--workspace-path",
        "/tmp/agentd/codex-sec",
        "--mcp-present",
        "true",
        "--daemon-url",
        &url,
    ]);
    assert!(
        runtime.status.success(),
        "runtime stderr: {}",
        String::from_utf8_lossy(&runtime.stderr)
    );
    assert!(String::from_utf8_lossy(&runtime.stdout).contains("blocked"));

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(
        seen,
        vec![
            "GET /api/agents/codex-sec/launch-env HTTP/1.1",
            "POST /api/agents/codex-sec/start HTTP/1.1",
            "POST /api/agents/codex-sec/runtime HTTP/1.1",
        ]
    );
}

#[test]
fn agent_cli_down_and_rebind_use_lifecycle_api_agents() {
    let (url, seen) = start_fake_daemon(vec![
        r#"{"ok":true,"action":"agent-down-kill","agent":{"name":"codex-sec","status":"offline"}}"#,
        r#"{"ok":true,"rebound":true,"agent":{"name":"codex-sec","status":"online"},"handle":{"address":"agentd-codex-sec:0.0"}}"#,
    ]);

    let down = agentctl(&["agent", "down", "codex-sec", "--daemon-url", &url]);
    assert!(
        down.status.success(),
        "down stderr: {}",
        String::from_utf8_lossy(&down.stderr)
    );
    assert!(String::from_utf8_lossy(&down.stdout).contains("agent-down-kill"));

    let rebind = agentctl(&["agent", "rebind", "codex-sec", "--daemon-url", &url]);
    assert!(
        rebind.status.success(),
        "rebind stderr: {}",
        String::from_utf8_lossy(&rebind.stderr)
    );
    assert!(String::from_utf8_lossy(&rebind.stdout).contains("agentd-codex-sec:0.0"));

    let seen = seen.lock().expect("seen lock").clone();
    assert_eq!(
        seen,
        vec![
            "POST /api/agents/codex-sec/down HTTP/1.1",
            "POST /api/agents/codex-sec/rebind HTTP/1.1",
        ]
    );
}
