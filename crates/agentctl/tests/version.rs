//! Scenario: --version prints package version
//! Scenario: --help lists the flow subcommand

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
fn agentctl_help_lists_flow_subcommand() {
    let out = Command::new(agentctl_bin())
        .arg("--help")
        .output()
        .expect("failed to spawn agentctl");
    assert!(out.status.success(), "non-zero exit: {:?}", out.status);
    let stdout = String::from_utf8_lossy(&out.stdout).to_lowercase();
    assert!(
        stdout.contains("flow"),
        "help did not mention the flow subcommand:\n{stdout}"
    );
}
