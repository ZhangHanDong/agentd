//! P0.8 T6: `agentctl run start` CLI behavior. Drives the built binary
//! end-to-end. Test names match `specs/workflow/p82-run-start-cli.spec.md`.

use std::process::{Command, Output};

/// The repo `workflows/` dir, resolved from the agentctl crate manifest.
fn workflows_dir() -> String {
    format!("{}/../../workflows", env!("CARGO_MANIFEST_DIR"))
}

/// Run the built `agentctl` binary with `args` and capture its output.
fn agentctl(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
        .args(args)
        .output()
        .expect("spawn agentctl")
}

#[test]
fn run_start_dry_run_draft_validates_and_prints_plan() {
    let out = agentctl(&[
        "run",
        "start",
        "--flow",
        "draft",
        "--workflows-dir",
        &workflows_dir(),
        "--dry-run",
        "ISSUE-1",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.contains("draft"), "names the draft flow: {stdout}");
    assert!(
        stdout.contains("propose_spec"),
        "lists the propose_spec node: {stdout}"
    );
}

#[test]
fn run_start_dry_run_execute_validates_and_prints_plan() {
    let out = agentctl(&[
        "run",
        "start",
        "--flow",
        "execute",
        "--workflows-dir",
        &workflows_dir(),
        "--dry-run",
        "SPEC-1",
    ]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(out.status.success(), "expected exit 0: {stdout}");
    assert!(
        stdout.contains("execute"),
        "names the execute flow: {stdout}"
    );
    assert!(
        stdout.contains("open_pr"),
        "lists the open_pr node: {stdout}"
    );
}

#[test]
fn run_start_live_unreachable_daemon_errors_cleanly() {
    // Bind a free port then close it -> a connect there is guaranteed refused,
    // so the live path fails fast and cleanly (never hangs).
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    let url = format!("http://127.0.0.1:{port}");

    let out = agentctl(&[
        "run",
        "start",
        "--flow",
        "draft",
        "--workflows-dir",
        &workflows_dir(),
        "--daemon-url",
        &url,
        "SPEC-1",
    ]);
    assert!(
        !out.status.success(),
        "an unreachable daemon is a non-zero error"
    );
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot reach"),
        "stderr should report the daemon is unreachable: {stderr}"
    );
}

#[test]
fn run_start_unknown_flow_is_error() {
    let out = agentctl(&["run", "start", "--flow", "bogus", "ISSUE-1"]);
    assert!(!out.status.success(), "an unknown --flow is a usage error");
}

#[test]
fn run_start_missing_workflow_file_is_error() {
    let out = agentctl(&[
        "run",
        "start",
        "--flow",
        "draft",
        "--workflows-dir",
        "/nonexistent/workflows",
        "--dry-run",
        "ISSUE-1",
    ]);
    assert!(!out.status.success(), "a missing workflow file is an error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("cannot read"),
        "stderr should report the unreadable file: {stderr}"
    );
}
