//! `agentctl runtime` client for the daemon's canonical native runtime API.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::path::Path;
use std::process::ExitCode;

use serde_json::{Value, json};

use crate::cli::{
    RuntimeCmd, RuntimeDaemonArgs, RuntimeInspectArgs, RuntimeInterruptArgs, RuntimeSendTextArgs,
    RuntimeShutdownArgs, RuntimeWaitArgs,
};

const EXIT_INVALID: u8 = 2;
const EXIT_DAEMON: u8 = 3;
const MAX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[must_use]
pub fn run(command: &RuntimeCmd) -> ExitCode {
    let result = match command {
        RuntimeCmd::Inspect(args) => inspect(args),
        RuntimeCmd::Wait(args) => wait(args),
        RuntimeCmd::SendText(args) => send_text(args),
        RuntimeCmd::Interrupt(args) => interrupt(args),
        RuntimeCmd::Shutdown(args) => shutdown(args),
    };
    match result {
        Ok(body) => {
            println!("{body}");
            ExitCode::SUCCESS
        }
        Err(RuntimeCliError::Invalid(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_INVALID)
        }
        Err(RuntimeCliError::Daemon(message)) => {
            eprintln!("error: {message}");
            ExitCode::from(EXIT_DAEMON)
        }
    }
}

fn inspect(args: &RuntimeInspectArgs) -> Result<String, RuntimeCliError> {
    request(
        &args.daemon,
        "GET",
        &format!("/api/runtime/sessions/{}", args.session_id),
        None,
    )
}

fn wait(args: &RuntimeWaitArgs) -> Result<String, RuntimeCliError> {
    request(
        &args.daemon,
        "GET",
        &format!(
            "/api/runtime/sessions/{}/wait?attempt_id={}&after_event_index={}&timeout_ms={}",
            args.session_id, args.attempt_id, args.after_event_index, args.timeout_ms
        ),
        None,
    )
}

fn send_text(args: &RuntimeSendTextArgs) -> Result<String, RuntimeCliError> {
    let admission = admission(&args.admission_file)?;
    request(
        &args.daemon,
        "POST",
        &format!("/api/runtime/sessions/{}/input/text", args.session_id),
        Some(json!({
            "admission": admission,
            "attempt_id": args.attempt_id,
            "idempotency_key": args.idempotency_key,
            "text": args.text,
            "submit": args.submit,
        })),
    )
}

fn interrupt(args: &RuntimeInterruptArgs) -> Result<String, RuntimeCliError> {
    let admission = admission(&args.admission_file)?;
    request(
        &args.daemon,
        "POST",
        &format!("/api/runtime/sessions/{}/interrupt", args.session_id),
        Some(json!({
            "admission": admission,
            "attempt_id": args.attempt_id,
            "idempotency_key": args.idempotency_key,
            "key": "ctrl_c",
            "repeat": 1,
        })),
    )
}

fn shutdown(args: &RuntimeShutdownArgs) -> Result<String, RuntimeCliError> {
    let admission = admission(&args.admission_file)?;
    request(
        &args.daemon,
        "POST",
        &format!("/api/runtime/sessions/{}/shutdown", args.session_id),
        Some(json!({
            "admission": admission,
            "attempt_id": args.attempt_id,
            "idempotency_key": args.idempotency_key,
            "graceful_timeout_ms": args.graceful_timeout_ms,
            "interrupt_timeout_ms": args.interrupt_timeout_ms,
            "reason": args.reason.as_str(),
        })),
    )
}

fn admission(path: &Path) -> Result<Value, RuntimeCliError> {
    let bytes = std::fs::read(path).map_err(|error| {
        RuntimeCliError::Invalid(format!("cannot read admission {}: {error}", path.display()))
    })?;
    serde_json::from_slice(&bytes).map_err(|error| {
        RuntimeCliError::Invalid(format!(
            "admission {} is invalid JSON: {error}",
            path.display()
        ))
    })
}

fn request(
    daemon: &RuntimeDaemonArgs,
    method: &str,
    path: &str,
    body: Option<Value>,
) -> Result<String, RuntimeCliError> {
    let address = daemon
        .daemon_url
        .strip_prefix("http://")
        .ok_or_else(|| RuntimeCliError::Invalid("runtime daemon URL must use http://".to_string()))?
        .trim_end_matches('/');
    if address.is_empty() || path.contains(['\r', '\n']) {
        return Err(RuntimeCliError::Invalid(
            "runtime daemon URL or request path is invalid".to_string(),
        ));
    }
    let body = body
        .map(|value| serde_json::to_string(&value))
        .transpose()
        .map_err(|error| RuntimeCliError::Invalid(error.to_string()))?
        .unwrap_or_default();
    let token = daemon
        .api_token
        .clone()
        .or_else(|| std::env::var("AGENTD_API_TOKEN").ok());
    let authorization = token.map_or_else(String::new, |token| {
        format!("Authorization: Bearer {token}\r\n")
    });
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: {address}\r\n{authorization}\
         Content-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let mut stream = TcpStream::connect(address).map_err(|error| {
        RuntimeCliError::Daemon(format!(
            "cannot reach runtime daemon at {}: {error}",
            daemon.daemon_url
        ))
    })?;
    stream
        .write_all(request.as_bytes())
        .map_err(|error| RuntimeCliError::Daemon(format!("request write failed: {error}")))?;
    let mut bytes = Vec::new();
    stream
        .take((MAX_RESPONSE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| RuntimeCliError::Daemon(format!("response read failed: {error}")))?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        return Err(RuntimeCliError::Daemon(
            "runtime daemon response exceeds the bound".to_string(),
        ));
    }
    let response = String::from_utf8_lossy(&bytes);
    let status = response
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|value| value.parse::<u16>().ok())
        .ok_or_else(|| RuntimeCliError::Daemon("malformed runtime daemon response".to_string()))?;
    let body = response
        .split_once("\r\n\r\n")
        .map_or("", |(_, body)| body)
        .to_string();
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(RuntimeCliError::Daemon(format!(
            "runtime daemon returned HTTP {status}: {body}"
        )))
    }
}

enum RuntimeCliError {
    Invalid(String),
    Daemon(String),
}
