use std::process::Command;

fn agentctl() -> Command {
    Command::new(env!("CARGO_BIN_EXE_agentctl"))
}

#[test]
fn enterprise_help_exposes_scale_compliance_and_recovery_commands() {
    let output = agentctl()
        .args(["enterprise", "--help"])
        .output()
        .expect("enterprise help");
    assert!(output.status.success());
    let help = String::from_utf8(output.stdout).expect("UTF-8 help");
    for command in [
        "status",
        "explain",
        "rollout",
        "rollout-observe",
        "zone-policy",
        "capacity",
        "replication-plan",
        "replica-ack",
        "tenant-key",
        "retention",
        "legal-hold",
        "legal-hold-release",
        "dr-checkpoint",
        "dr-drill",
        "load-model",
        "service-level",
    ] {
        assert!(help.contains(command), "missing enterprise command {command}");
    }
}

#[test]
fn enterprise_status_rejects_implicit_plain_http() {
    let output = agentctl()
        .args([
            "enterprise",
            "status",
            "--daemon-url",
            "http://127.0.0.1:8787",
            "--api-token",
            "operator-secret",
        ])
        .output()
        .expect("enterprise status");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("--allow-loopback-http"));
}

#[test]
fn enterprise_status_requires_an_operator_token_before_connecting() {
    let output = agentctl()
        .env_remove("AGENTD_API_TOKEN")
        .args([
            "enterprise",
            "status",
            "--daemon-url",
            "http://127.0.0.1:1",
            "--allow-loopback-http",
        ])
        .output()
        .expect("enterprise status");
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("bearer token is required"));
}
