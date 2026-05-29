//! Tests for `agentctl flow validate`. Names match the spec `Test:` selectors
//! in specs/core/p2-node-graph-validate.spec.md.

use std::process::Command;

use tempfile::TempDir;

fn agentctl_bin() -> std::path::PathBuf {
    env!("CARGO_BIN_EXE_agentctl").into()
}

/// Write `src` to a `.dot` file in a fresh temp dir and run `flow validate` on it.
fn validate(src: &str) -> (std::process::Output, TempDir) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("flow.dot");
    std::fs::write(&path, src).expect("write dot");
    let out = Command::new(agentctl_bin())
        .args(["flow", "validate"])
        .arg(&path)
        .output()
        .expect("failed to spawn agentctl");
    (out, dir)
}

const VALID: &str = r#"digraph m {
    "start" [shape=Mdiamond];
    "work"  [handler=tool, cmd="echo hi"];
    "end"   [shape=Msquare];
    "start" -> "work";
    "work"  -> "end";
}"#;

#[test]
fn agentctl_flow_validate_succeeds_on_valid_dot() {
    let (out, _dir) = validate(VALID);
    assert!(
        out.status.success(),
        "valid dot should exit 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn agentctl_flow_validate_fails_on_invalid_dot_with_exit_2() {
    // No terminal node → validation fails.
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work"  [handler=tool];
        "start" -> "work";
    }"#;
    let (out, _dir) = validate(src);
    assert_eq!(out.status.code(), Some(2), "invalid dot should exit 2");
}

#[test]
fn agentctl_flow_validate_lists_all_violations_in_stderr() {
    // Two violations at once: no terminal node AND an unknown handler.
    let src = r#"digraph m {
        "start" [shape=Mdiamond];
        "work"  [handler=stack.manager_loop];
        "start" -> "work";
    }"#;
    let (out, _dir) = validate(src);
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("terminal"),
        "stderr missing the terminal violation: {stderr}"
    );
    assert!(
        stderr.contains("stack.manager_loop"),
        "stderr missing the unknown-handler violation: {stderr}"
    );
}
