use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn repo_root() -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path
}

fn script_path() -> PathBuf {
    repo_root()
        .join("scripts")
        .join("agentd_matrix_client_bridge_service_smoke.sh")
}

fn run_script(args: &[&str]) -> Output {
    run_script_with_env(args, &[])
}

fn run_script_with_env(args: &[&str], envs: &[(&str, &str)]) -> Output {
    let mut command = Command::new("bash");
    command
        .arg(script_path())
        .args(args)
        .current_dir(repo_root())
        .env_remove("AGENTD_REAL_MATRIX_SERVICE_SMOKE")
        .env_remove("AGENTD_MATRIX_HOMESERVER_URL")
        .env_remove("AGENTD_MATRIX_USERNAME")
        .env_remove("AGENTD_MATRIX_PASSWORD")
        .env_remove("AGENTD_MATRIX_USER_ID")
        .env_remove("AGENTD_MATRIX_ACCESS_TOKEN")
        .env_remove("AGENTD_MATRIX_DEVICE_ID")
        .env_remove("AGENTD_MATRIX_AGENT_PASSWORD_SECRET")
        .env_remove("AGENTD_MATRIX_REGISTRATION_TOKEN");
    for (key, value) in envs {
        command.env(key, value);
    }
    command.output().expect("run Matrix service smoke script")
}

#[test]
fn matrix_service_smoke_dry_run_prints_plan_without_side_effects() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let state_dir_arg = state_dir.to_string_lossy().to_string();
    let out = run_script(&[
        "--dry-run",
        "--run-id",
        "matrix-service-dry-run",
        "--state-dir",
        &state_dir_arg,
        "--matrix-homeserver-url",
        "http://matrix.example",
        "--matrix-username",
        "agentd-bot",
        "--matrix-password",
        "bot-secret",
        "--matrix-access-token",
        "token-like-secret",
        "--matrix-agent-password-secret",
        "puppet-secret",
        "--matrix-registration-token",
        "registration-secret",
    ]);

    assert!(
        out.status.success(),
        "dry-run exits 0; stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("agentd matrix-client-bridge-preflight"),
        "{stdout}"
    );
    assert!(
        stdout.contains("agentd matrix-client-bridge-service"),
        "{stdout}"
    );
    for artifact in ["preflight.out", "service.out", "summary.txt"] {
        assert!(
            stdout.contains(artifact),
            "stdout should mention {artifact}: {stdout}"
        );
    }
    for marker in [
        "password: set (redacted)",
        "access_token: set (redacted)",
        "agent_password_secret: set (redacted)",
        "registration_token: set (redacted)",
    ] {
        assert!(
            stdout.contains(marker),
            "stdout should mention {marker}: {stdout}"
        );
    }
    for secret in [
        "bot-secret",
        "token-like-secret",
        "puppet-secret",
        "registration-secret",
    ] {
        assert!(
            !stdout.contains(secret),
            "dry-run must not print secret value {secret}: {stdout}"
        );
    }
    assert!(
        !state_dir.exists(),
        "dry-run should not create the state directory"
    );
}

#[test]
fn matrix_service_smoke_execute_requires_explicit_opt_in() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let fake_agentd = temp.path().join("agentd");
    write_fake_tool(&fake_agentd, "echo unused\n");
    let out = run_script(&[
        "--execute",
        "--skip-build",
        "--agentd-bin",
        fake_agentd.to_string_lossy().as_ref(),
        "--state-dir",
        state_dir.to_string_lossy().as_ref(),
        "--matrix-homeserver-url",
        "http://matrix.example",
        "--matrix-username",
        "agentd-bot",
        "--matrix-password",
        "bot-secret",
    ]);

    assert!(!out.status.success(), "execute without opt-in fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("AGENTD_REAL_MATRIX_SERVICE_SMOKE=1"),
        "stderr should name the opt-in env var: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "execute without opt-in should not create the state directory"
    );
}

#[test]
fn matrix_service_smoke_preflight_only_requires_login_mode() {
    let temp = tempfile::tempdir().expect("tempdir");
    let state_dir = temp.path().join("state");
    let fake_agentd = temp.path().join("agentd");
    write_fake_tool(&fake_agentd, "echo unused\n");
    let out = run_script(&[
        "--preflight-only",
        "--skip-build",
        "--agentd-bin",
        fake_agentd.to_string_lossy().as_ref(),
        "--state-dir",
        state_dir.to_string_lossy().as_ref(),
        "--matrix-homeserver-url",
        "http://matrix.example",
    ]);

    assert!(!out.status.success(), "missing Matrix SDK auth mode fails");
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("username/password") && stderr.contains("user-id/access-token"),
        "stderr should name supported login modes: {stderr}"
    );
    assert!(
        !state_dir.exists(),
        "preflight-only should not create the state directory"
    );
}

#[test]
fn matrix_service_smoke_execute_invokes_preflight_then_service_and_writes_evidence() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_agentd = temp.path().join("agentd");
    let args_file = temp.path().join("agentd.args");
    write_fake_service_agentd(&fake_agentd);
    let state_dir = temp.path().join("state");
    let out = run_service_execute_smoke(&fake_agentd, &args_file, &state_dir);

    assert!(
        out.status.success(),
        "fake execute succeeds; stdout: {}; stderr: {}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    let args = fs::read_to_string(&args_file).expect("read fake agentd args");
    assert_preflight_before_service(&args);
    assert_service_smoke_args(&args);
    assert_service_smoke_evidence(&state_dir);
}

fn write_fake_service_agentd(path: &Path) {
    write_fake_tool(
        path,
        r#"printf 'call:%s\n' "$*" >> "$FAKE_AGENTD_ARGS"
case "${1:-}" in
  matrix-client-bridge-preflight)
    echo 'matrix-client-bridge-preflight: homeserver=http://matrix.test versions=v1.12 whoami_user_id=not_checked iterations=1 puppet_accounts_configured=false'
    ;;
  matrix-client-bridge-service)
    shift
    state=''
    while [[ $# -gt 0 ]]; do
      case "$1" in
        --state)
          state="${2:?missing state}"
          shift 2
          ;;
        *)
          shift
          ;;
      esac
    done
    mkdir -p "$(dirname "$state")"
    printf '{"nextFromSeq":7}\n' > "$state"
    echo 'matrix-client-bridge-service: iterations=1 registered_rooms=1 inbound_forwarded=0 outbound_sent=1 next_from_seq=7'
    ;;
  *)
    echo "unexpected subcommand: ${1:-}" >&2
    exit 7
    ;;
esac
"#,
    );
}

fn run_service_execute_smoke(fake_agentd: &Path, args_file: &Path, state_dir: &Path) -> Output {
    run_script_with_env(
        &[
            "--execute",
            "--skip-build",
            "--agentd-bin",
            fake_agentd.to_string_lossy().as_ref(),
            "--run-id",
            "matrix-service-execute",
            "--state-dir",
            state_dir.to_string_lossy().as_ref(),
            "--agentd-api",
            "http://127.0.0.1:8787",
            "--matrix-homeserver-url",
            "http://matrix.test",
            "--matrix-username",
            "agentd-bot",
            "--matrix-password",
            "bot-secret",
            "--matrix-agent",
            "codex-worker",
        ],
        &[
            ("AGENTD_REAL_MATRIX_SERVICE_SMOKE", "1"),
            ("FAKE_AGENTD_ARGS", args_file.to_string_lossy().as_ref()),
        ],
    )
}

fn assert_preflight_before_service(args: &str) {
    let preflight_pos = args
        .find("call:matrix-client-bridge-preflight")
        .expect("preflight invocation");
    let service_pos = args
        .find("call:matrix-client-bridge-service")
        .expect("service invocation");
    assert!(
        preflight_pos < service_pos,
        "preflight should run before service: {args}"
    );
}

fn assert_service_smoke_args(args: &str) {
    assert!(
        args.contains("--matrix-homeserver-url") && args.contains("http://matrix.test"),
        "fake agentd should receive the homeserver URL: {args}"
    );
    assert!(
        args.contains("--matrix-username") && args.contains("agentd-bot"),
        "fake agentd should receive username credentials: {args}"
    );
    assert!(
        args.contains("--matrix-password") && args.contains("bot-secret"),
        "fake agentd should receive password credentials: {args}"
    );
    assert!(
        !args.contains("--features"),
        "agentd command should not receive cargo feature flags: {args}"
    );
}

fn assert_service_smoke_evidence(state_dir: &Path) {
    for artifact in [
        "preflight.out",
        "preflight.err",
        "service.out",
        "service.err",
    ] {
        assert!(
            state_dir.join(artifact).exists(),
            "{artifact} should be captured"
        );
    }
    let service_out = fs::read_to_string(state_dir.join("service.out")).expect("read service.out");
    assert!(
        service_out.contains("matrix-client-bridge-service: iterations=1"),
        "{service_out}"
    );
    let summary = fs::read_to_string(state_dir.join("summary.txt")).expect("read summary");
    assert!(summary.contains("result: finished"), "{summary}");
    assert!(summary.contains("password: set (redacted)"), "{summary}");
    assert!(
        !summary.contains("bot-secret"),
        "summary must not print Matrix password: {summary}"
    );
    assert!(
        state_dir.join("matrix-client-bridge-state.json").exists(),
        "service smoke should require cursor state evidence"
    );
    assert!(
        !state_dir.join("daemon.log").exists(),
        "service smoke should not start the daemon"
    );
}

fn write_fake_tool(path: &Path, body: &str) {
    fs::write(
        path,
        format!("#!/usr/bin/env bash\nset -euo pipefail\n{body}"),
    )
    .expect("write fake tool");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mut perms = fs::metadata(path).expect("metadata").permissions();
        perms.set_mode(0o755);
        fs::set_permissions(path, perms).expect("chmod fake tool");
    }
}
