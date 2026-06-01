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
fn run_start_live_path_is_deferred_error() {
    let out = agentctl(&[
        "run",
        "start",
        "--flow",
        "execute",
        "--workflows-dir",
        &workflows_dir(),
        "SPEC-1",
    ]);
    assert!(!out.status.success(), "live path must be a non-zero error");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("P0.9"),
        "stderr should say live execution is deferred to P0.9: {stderr}"
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
