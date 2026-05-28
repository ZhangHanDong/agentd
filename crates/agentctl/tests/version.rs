//! Scenario: --version prints package version
//! Scenario: --help lists no real subcommands yet

use std::process::Command;

fn agentctl_bin() -> std::path::PathBuf {
    // cargo provides CARGO_BIN_EXE_<name> for binary crates in their own tests
    env!("CARGO_BIN_EXE_agentctl").into()
}

#[test]
fn agentctl_version_matches_cargo_metadata() {
    let out = Command::new(agentctl_bin())
        .arg("--version")
        .output()
        .expect("failed to spawn agentctl");
    assert!(out.status.success(), "non-zero exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.trim() == "agentctl 0.0.0",
        "unexpected version line: {stdout:?}",
    );
}

#[test]
fn agentctl_help_lists_only_placeholder() {
    let out = Command::new(agentctl_bin())
        .arg("--help")
        .output()
        .expect("failed to spawn agentctl");
    assert!(out.status.success());
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(
        stdout.contains("noop"),
        "help did not mention placeholder noop subcommand:\n{stdout}"
    );
    assert!(
        !stdout.contains("flow"),
        "help unexpectedly contains flow subcommand: {stdout}"
    );
    assert!(
        !stdout.contains("run "),
        "help unexpectedly contains run subcommand: {stdout}"
    );
}
