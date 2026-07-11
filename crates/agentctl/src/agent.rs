//! `agentctl agent` — thin CLI client for the daemon's local agent registry.

use std::process::ExitCode;

use crate::cli::{
    AgentCmd, AgentDownArgs, AgentHeartbeatArgs, AgentInspectArgs, AgentLaunchEnvArgs,
    AgentListArgs, AgentOfflineArgs, AgentRebindArgs, AgentRegisterArgs, AgentRuntimeArgs,
    AgentStartArgs,
};

const EXIT_DAEMON: u8 = 3;
const MAX_RESPONSE_BYTES: usize = 64 * 1024;

#[must_use]
pub fn run(cmd: &AgentCmd) -> ExitCode {
    match cmd {
        AgentCmd::Ls(args) => list(args),
        AgentCmd::Inspect(args) => inspect(args),
        AgentCmd::LaunchEnv(args) => launch_env(args),
        AgentCmd::Start(args) => start(args),
        AgentCmd::Down(args) => down(args),
        AgentCmd::Rebind(args) => rebind(args),
        AgentCmd::Runtime(args) => runtime(args),
        AgentCmd::Register(args) => register(args),
        AgentCmd::Heartbeat(args) => heartbeat(args),
        AgentCmd::Offline(args) => offline(args),
    }
}

fn list(args: &AgentListArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "GET",
        "/api/agents",
        None,
        &operator_headers(args.api_token.as_deref()),
    )
}

fn inspect(args: &AgentInspectArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "GET",
        &format!("/api/agents/{}", path_segment(&args.name)),
        None,
        &operator_headers(args.api_token.as_deref()),
    )
}

fn launch_env(args: &AgentLaunchEnvArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "GET",
        &format!("/api/agents/{}/launch-env", path_segment(&args.name)),
        None,
        &operator_headers(args.api_token.as_deref()),
    )
}

fn start(args: &AgentStartArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/start", path_segment(&args.name)),
        Some("{}"),
        &operator_headers(args.api_token.as_deref()),
    )
}

fn down(args: &AgentDownArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/down", path_segment(&args.name)),
        Some("{}"),
        &operator_headers(args.api_token.as_deref()),
    )
}

fn rebind(args: &AgentRebindArgs) -> ExitCode {
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/rebind", path_segment(&args.name)),
        Some("{}"),
        &operator_headers(args.api_token.as_deref()),
    )
}

fn runtime(args: &AgentRuntimeArgs) -> ExitCode {
    let mut fields = Vec::new();
    if args.blocked {
        fields.push(json_bool_field("blocked", true));
    }
    push_opt(&mut fields, "reason", args.reason.as_deref());
    push_bool_opt(&mut fields, "activeNow", args.active_now);
    push_i64_opt(&mut fields, "activeDurationSec", args.active_duration_sec);
    push_i64_opt(&mut fields, "idleDurationSec", args.idle_duration_sec);
    push_i64_opt(
        &mut fields,
        "lastTmuxActivitySec",
        args.last_tmux_activity_sec,
    );
    push_opt(&mut fields, "workspacePath", args.workspace_path.as_deref());
    push_bool_opt(&mut fields, "mcpPresent", args.mcp_present);
    let body = json_object(&fields);
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/runtime", path_segment(&args.name)),
        Some(&body),
        &agent_headers(args.agent_token.as_deref()),
    )
}

fn register(args: &AgentRegisterArgs) -> ExitCode {
    let mut fields = vec![json_field("name", &args.name)];
    push_opt(&mut fields, "role", args.role.as_deref());
    push_opt(&mut fields, "capability", args.capability.as_deref());
    push_opt(&mut fields, "runtime", args.runtime.as_deref());
    push_opt(&mut fields, "model", args.model.as_deref());
    push_opt(&mut fields, "tmux_target", args.tmux_target.as_deref());
    push_opt(&mut fields, "home_dir", args.home_dir.as_deref());
    push_opt(&mut fields, "workdir", args.workdir.as_deref());
    push_opt(&mut fields, "state_dir", args.state_dir.as_deref());
    push_opt(&mut fields, "server", args.server.as_deref());
    let body = json_object(&fields);
    request_and_print(
        &args.daemon_url,
        "POST",
        "/api/agents",
        Some(&body),
        &agent_headers(args.agent_token.as_deref()),
    )
}

fn heartbeat(args: &AgentHeartbeatArgs) -> ExitCode {
    let mut fields = Vec::new();
    push_opt(&mut fields, "server", args.server.as_deref());
    push_opt(&mut fields, "tmux_target", args.tmux_target.as_deref());
    push_opt(
        &mut fields,
        "workspace_path",
        args.workspace_path.as_deref(),
    );
    let body = json_object(&fields);
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/heartbeat", path_segment(&args.name)),
        Some(&body),
        &agent_headers(args.agent_token.as_deref()),
    )
}

fn offline(args: &AgentOfflineArgs) -> ExitCode {
    let mut fields = Vec::new();
    push_opt(&mut fields, "reason", args.reason.as_deref());
    if args.no_clear_tmux {
        fields.push("\"clear_tmux\":false".to_string());
    }
    let body = json_object(&fields);
    request_and_print(
        &args.daemon_url,
        "POST",
        &format!("/api/agents/{}/offline", path_segment(&args.name)),
        Some(&body),
        &agent_headers(args.agent_token.as_deref()),
    )
}

fn request_and_print(
    daemon_url: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(String, String)],
) -> ExitCode {
    match http_request(daemon_url, method, path, body, headers) {
        Ok((code, response_body)) if (200..300).contains(&code) => {
            println!("{response_body}");
            ExitCode::SUCCESS
        }
        Ok((code, response_body)) => {
            eprintln!("error: daemon rejected the agent request (HTTP {code}): {response_body}");
            ExitCode::from(EXIT_DAEMON)
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::from(EXIT_DAEMON)
        }
    }
}

fn http_request(
    daemon_url: &str,
    method: &str,
    path: &str,
    body: Option<&str>,
    headers: &[(String, String)],
) -> Result<(u16, String), String> {
    use std::fmt::Write as _;
    use std::io::Write;
    use std::net::TcpStream;

    let addr = daemon_url
        .strip_prefix("http://")
        .unwrap_or(daemon_url)
        .trim_end_matches('/');
    let mut stream = TcpStream::connect(addr)
        .map_err(|e| format!("cannot reach daemon at {daemon_url}: {e}"))?;
    let body = body.unwrap_or("");
    let mut header_lines = String::new();
    for (key, value) in headers {
        let _ = write!(&mut header_lines, "{key}: {value}\r\n");
    }
    let request = if method == "GET" {
        format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\n{header_lines}Connection: close\r\n\r\n")
    } else {
        format!(
            "{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{header_lines}Content-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    };
    stream
        .write_all(request.as_bytes())
        .map_err(|e| format!("writing to daemon failed: {e}"))?;
    read_response(stream)
}

fn read_response(stream: impl std::io::Read) -> Result<(u16, String), String> {
    use std::io::Read;
    let mut buf = Vec::new();
    stream
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut buf)
        .map_err(|e| format!("reading from daemon failed: {e}"))?;
    if buf.len() > MAX_RESPONSE_BYTES {
        return Err(format!(
            "daemon response exceeds {MAX_RESPONSE_BYTES} bytes"
        ));
    }
    let response = String::from_utf8_lossy(&buf);
    let code = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|c| c.parse::<u16>().ok())
        .ok_or_else(|| "malformed daemon response".to_string())?;
    let body = response
        .split_once("\r\n\r\n")
        .map_or("", |(_, b)| b)
        .to_string();
    Ok((code, body))
}

fn operator_headers(flag: Option<&str>) -> Vec<(String, String)> {
    clean_token(flag)
        .or_else(|| env_token("AGENTD_API_TOKEN"))
        .map(|token| vec![("Authorization".to_string(), format!("Bearer {token}"))])
        .unwrap_or_default()
}

fn agent_headers(flag: Option<&str>) -> Vec<(String, String)> {
    clean_token(flag)
        .or_else(|| env_token("AGENTD_AGENT_TOKEN"))
        .or_else(|| env_token("AGENTCHAT_AGENT_TOKEN"))
        .map(|token| vec![("X-Agent-Token".to_string(), token)])
        .unwrap_or_default()
}

fn env_token(name: &str) -> Option<String> {
    std::env::var(name).ok().and_then(|v| clean_token(Some(&v)))
}

fn clean_token(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn push_opt(fields: &mut Vec<String>, key: &str, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|v| !v.is_empty()) {
        fields.push(json_field(key, value));
    }
}

fn push_bool_opt(fields: &mut Vec<String>, key: &str, value: Option<bool>) {
    if let Some(value) = value {
        fields.push(json_bool_field(key, value));
    }
}

fn push_i64_opt(fields: &mut Vec<String>, key: &str, value: Option<i64>) {
    if let Some(value) = value {
        fields.push(format!("{}:{value}", json_string(key)));
    }
}

fn json_object(fields: &[String]) -> String {
    format!("{{{}}}", fields.join(","))
}

fn json_bool_field(key: &str, value: bool) -> String {
    format!("{}:{value}", json_string(key))
}

fn json_field(key: &str, value: &str) -> String {
    format!("{}:{}", json_string(key), json_string(value))
}

fn json_string(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{:04x}", c as u32);
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn path_segment(value: &str) -> String {
    value.replace(' ', "%20")
}
